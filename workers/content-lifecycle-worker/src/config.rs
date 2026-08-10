//! Validated start-up configuration for `content-lifecycle-worker`.
//!
//! One binary, five deployed roles (RS-10, extended by E D-8). `AEX_MODE` selects
//! the role, and the required variable set is a function of it: `delete` is the
//! only mode that may hold the object-delete capability, and `reconcile` is the
//! only mode that reads the inventory bucket. A mode whose variables are absent
//! refuses to start.
//!
//! `uploadexpiry` is the fifth role. It is separate from `expiry` rather than
//! folded into it because it is the only role that touches `regional-registry` and
//! the only non-`delete` role that calls S3 at all — it issues
//! `AbortMultipartUpload`. Splitting it is what lets the two IAM policies differ:
//! the grant-expiry role holds no registry access and no S3 access whatsoever, and
//! **neither** ever holds `s3:DeleteObject` (E D-4).

use aex_regional_http::capability::Capability as _;
use aex_regional_http::config::{
    Lookup, RegionalHttpConfigError, bounded_u64, forbidden, one_of, optional, plane_name,
    queue_url, region, required,
};
use aex_wire::types::Region;

/// The deployable this configuration belongs to.
pub const DEPLOYABLE: &str = "content-lifecycle-worker";

/// Which of the four deployed roles this process is.
pub const MODE: &str = "AEX_MODE";
/// Deployment plane: `dev` or `prd`.
pub const PLANE: &str = "AEX_PLANE";
/// The region this process is pinned to.
pub const REGION: &str = "AEX_REGION";
/// The release digest reported in every record.
pub const RELEASE_DIGEST: &str = "AEX_RELEASE_DIGEST";
/// The `regional-content` table.
pub const CONTENT_TABLE: &str = "AEX_CONTENT_TABLE";
/// The `regional-registry` table.
pub const REGISTRY_TABLE: &str = "AEX_REGISTRY_TABLE";
/// The `regional-work` table.
pub const WORK_TABLE: &str = "AEX_WORK_TABLE";
/// The regional content bucket.
pub const CONTENT_BUCKET: &str = "AEX_CONTENT_BUCKET";
/// The account that must own the content bucket.
pub const CONTENT_BUCKET_OWNER: &str = "AEX_CONTENT_BUCKET_OWNER";
/// The deletion-denial projection table.
pub const DENIAL_PROJECTION_TABLE: &str = "AEX_DENIAL_PROJECTION_TABLE";
/// Staged-orphan grace in hours.
pub const GC_STAGE_GRACE_HOURS: &str = "AEX_GC_STAGE_GRACE_HOURS";
/// Pending-upload grace in hours.
pub const UPLOAD_GRACE_HOURS: &str = "AEX_UPLOAD_GRACE_HOURS";
/// Number of sharded download-grant due partitions read per expiry tick.
pub const EXPIRY_SCAN_SHARDS: &str = "AEX_EXPIRY_SCAN_SHARDS";
/// Maximum expired grants read from each shard per expiry tick.
pub const EXPIRY_PAGE_ITEMS: &str = "AEX_EXPIRY_PAGE_ITEMS";
/// Mark-phase page size.
pub const MARK_PAGE_ITEMS: &str = "AEX_MARK_PAGE_ITEMS";
/// Sweep-phase page size.
pub const SWEEP_PAGE_ITEMS: &str = "AEX_SWEEP_PAGE_ITEMS";
/// The content delete queue; `delete` mode only.
pub const CONTENT_QUEUE_URL: &str = "AEX_CONTENT_QUEUE_URL";
/// The content delete dead-letter queue; `delete` mode only.
pub const CONTENT_DLQ_URL: &str = "AEX_CONTENT_DLQ_URL";
/// The S3 Inventory bucket; `reconcile` mode only.
pub const INVENTORY_BUCKET: &str = "AEX_INVENTORY_BUCKET";
/// The capability set this process declares, comma-separated.
///
/// A role that holds none declares [`NO_CAPABILITIES`] rather than an empty
/// string: "declared nothing" and "forgot to declare" must not be the same
/// value, because only one of them is a deliberate statement.
pub const DECLARED_CAPABILITIES: &str = "AEX_DECLARED_CAPABILITIES";

/// The explicit declaration that a role holds no privileged capability.
pub const NO_CAPABILITIES: &str = "none";

/// The variables every mode requires.
pub const REQUIRED_COMMON: [&str; 13] = [
    MODE,
    PLANE,
    REGION,
    RELEASE_DIGEST,
    CONTENT_TABLE,
    REGISTRY_TABLE,
    WORK_TABLE,
    CONTENT_BUCKET,
    CONTENT_BUCKET_OWNER,
    DENIAL_PROJECTION_TABLE,
    GC_STAGE_GRACE_HOURS,
    UPLOAD_GRACE_HOURS,
    DECLARED_CAPABILITIES,
];

/// Variables this binary must never be bound to.
pub const FORBIDDEN: [(&str, &str); 2] = [
    (
        "AEX_SECRET_KMS_KEY_ARN",
        "content lifecycle never decrypts a secret",
    ),
    (
        "AEX_SESSION_TABLE",
        "content lifecycle holds no session binding",
    ),
];

/// The five deployed roles of the one binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Expires lapsed download grants.
    Expiry,
    /// Expires pending uploads and aborts the multipart uploads they name.
    UploadExpiry,
    /// Confirms staged orphans after the grace window.
    Reconcile,
    /// Walks reachability and stages what nothing points at.
    MarkSweep,
    /// The only role that may delete an object.
    Delete,
}

impl Mode {
    /// The wire spelling of every mode.
    pub const ALL: [&'static str; 5] =
        ["expiry", "uploadexpiry", "reconcile", "marksweep", "delete"];

    /// Resolves a mode name.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "expiry" => Some(Self::Expiry),
            "uploadexpiry" => Some(Self::UploadExpiry),
            "reconcile" => Some(Self::Reconcile),
            "marksweep" => Some(Self::MarkSweep),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }

    /// The stable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Expiry => "expiry",
            Self::UploadExpiry => "uploadexpiry",
            Self::Reconcile => "reconcile",
            Self::MarkSweep => "marksweep",
            Self::Delete => "delete",
        }
    }

    /// Whether this role performs the one destructive object effect.
    #[must_use]
    pub const fn deletes_objects(self) -> bool {
        matches!(self, Self::Delete)
    }

    /// Whether this role constructs an S3 client at all.
    ///
    /// `uploadexpiry` needs one for `AbortMultipartUpload` and `HeadObject`; it
    /// still cannot delete an object, because [`Mode::deletes_objects`] is false
    /// and the capability check refuses the pairing at start-up.
    #[must_use]
    pub const fn reaches_objects(self) -> bool {
        matches!(self, Self::Delete | Self::UploadExpiry)
    }
}

/// Resolved configuration. Nothing here has a default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// The deployed role.
    pub mode: Mode,
    /// Deployment plane.
    pub plane: aex_identity_domain::assertion::Plane,
    /// Pinned region.
    pub region: Region,
    /// Release digest.
    pub release_digest: String,
    /// `regional-content` table.
    pub content_table: String,
    /// `regional-registry` table.
    pub registry_table: String,
    /// `regional-work` table.
    pub work_table: String,
    /// Regional content bucket.
    pub content_bucket: String,
    /// Expected content-bucket owner account.
    pub content_bucket_owner: String,
    /// Deletion-denial projection table.
    pub denial_projection_table: String,
    /// Staged-orphan grace in hours.
    pub gc_stage_grace_hours: u64,
    /// Pending-upload grace in hours.
    pub upload_grace_hours: u64,
    /// Due partitions read per scheduled invocation, in the two expiry modes.
    pub expiry_scan_shards: Option<u16>,
    /// Maximum rows read from each due partition, in the two expiry modes.
    pub expiry_page_items: Option<u32>,
    /// Mark-phase page size, in `marksweep` mode.
    pub mark_page_items: Option<u64>,
    /// Sweep-phase page size, in `marksweep` mode.
    pub sweep_page_items: Option<u64>,
    /// Content delete queue, in `delete` mode.
    pub content_queue_url: Option<String>,
    /// Content delete dead-letter queue, in `delete` mode.
    pub content_dlq_url: Option<String>,
    /// S3 Inventory bucket, in `reconcile` mode.
    pub inventory_bucket: Option<String>,
    /// The capability ids this process declares.
    pub declared_capabilities: Vec<String>,
}

impl Config {
    /// Reads and validates the configuration from the process environment.
    ///
    /// # Errors
    ///
    /// Returns the first [`RegionalHttpConfigError`], naming the offending variable.
    pub fn from_env() -> Result<Self, RegionalHttpConfigError> {
        Self::read(&aex_regional_http::config::Environment)
    }

    /// Reads and validates the configuration from an arbitrary lookup.
    ///
    /// # Errors
    ///
    /// Identical to [`Config::from_env`].
    ///
    /// # Panics
    ///
    /// Never: the numeric conversions follow stricter admitted bounds than
    /// their destination integer types.
    #[allow(clippy::too_many_lines, reason = "one arm per mode-specific variable")]
    pub fn read<L: Lookup + ?Sized>(lookup: &L) -> Result<Self, RegionalHttpConfigError> {
        for (name, reason) in FORBIDDEN {
            forbidden(lookup, name, DEPLOYABLE, reason)?;
        }
        let raw_mode = one_of(lookup, MODE, &Mode::ALL)?;
        let mode = Mode::parse(&raw_mode).ok_or_else(|| RegionalHttpConfigError::Invalid {
            name: MODE,
            reason: format!("expected one of {:?}, got `{raw_mode}`", Mode::ALL),
        })?;
        let plane = plane_name(lookup, PLANE)?;
        let region = region(lookup, REGION)?;
        let raw_capabilities = required(lookup, DECLARED_CAPABILITIES)?;
        let declared_capabilities = if raw_capabilities == NO_CAPABILITIES {
            Vec::new()
        } else {
            raw_capabilities
                .split(',')
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        };

        // The capability half of RS-09: `delete` is the only mode that may hold
        // the object-delete capability, and it may not run without it. Both
        // directions are refused at start-up rather than at the destructive call.
        let holds_delete = declared_capabilities
            .iter()
            .any(|id| id == aex_regional_http::capability::ContentObjectDelete::ID);
        if mode.deletes_objects() && !holds_delete {
            return Err(RegionalHttpConfigError::Invalid {
                name: DECLARED_CAPABILITIES,
                reason: format!(
                    "`delete` mode requires `{}`",
                    aex_regional_http::capability::ContentObjectDelete::ID
                ),
            });
        }
        if !mode.deletes_objects() && holds_delete {
            return Err(RegionalHttpConfigError::Forbidden {
                name: DECLARED_CAPABILITIES,
                deployable: DEPLOYABLE,
                reason: "only `delete` mode may hold the object-delete capability",
            });
        }

        let (content_queue_url, content_dlq_url) = if mode.deletes_objects() {
            (
                Some(queue_url(lookup, CONTENT_QUEUE_URL, region)?),
                Some(queue_url(lookup, CONTENT_DLQ_URL, region)?),
            )
        } else {
            (None, None)
        };
        let (mark_page_items, sweep_page_items) = if mode == Mode::MarkSweep {
            (
                Some(bounded_u64(lookup, MARK_PAGE_ITEMS, 1, 10_000)?),
                Some(bounded_u64(lookup, SWEEP_PAGE_ITEMS, 1, 10_000)?),
            )
        } else {
            (None, None)
        };
        let inventory_bucket = if mode == Mode::Reconcile {
            optional(lookup, INVENTORY_BUCKET)
        } else {
            None
        };
        let (expiry_scan_shards, expiry_page_items) =
            if matches!(mode, Mode::Expiry | Mode::UploadExpiry) {
                (
                    Some(
                        u16::try_from(bounded_u64(lookup, EXPIRY_SCAN_SHARDS, 1, 64)?)
                            .expect("the admitted bound fits u16"),
                    ),
                    Some(
                        u32::try_from(bounded_u64(lookup, EXPIRY_PAGE_ITEMS, 1, 100)?)
                            .expect("the admitted bound fits u32"),
                    ),
                )
            } else {
                (None, None)
            };

        Ok(Self {
            mode,
            plane,
            region,
            release_digest: required(lookup, RELEASE_DIGEST)?,
            content_table: required(lookup, CONTENT_TABLE)?,
            registry_table: required(lookup, REGISTRY_TABLE)?,
            work_table: required(lookup, WORK_TABLE)?,
            content_bucket: required(lookup, CONTENT_BUCKET)?,
            content_bucket_owner: required(lookup, CONTENT_BUCKET_OWNER)?,
            denial_projection_table: required(lookup, DENIAL_PROJECTION_TABLE)?,
            gc_stage_grace_hours: bounded_u64(lookup, GC_STAGE_GRACE_HOURS, 1, 720)?,
            upload_grace_hours: bounded_u64(lookup, UPLOAD_GRACE_HOURS, 1, 720)?,
            expiry_scan_shards,
            expiry_page_items,
            mark_page_items,
            sweep_page_items,
            content_queue_url,
            content_dlq_url,
            inventory_bucket,
            declared_capabilities,
        })
    }
}
