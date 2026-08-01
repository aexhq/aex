//! Object addressing, and the constants the bucket policy has to agree with.
//!
//! A key is a pure function of `(workspace, digest)`. That is what makes a
//! conditional create idempotent — the same body always lands on the same key —
//! and what makes a delete provably target the exact body a sweep decided on.
//! There is no random suffix and no version id, because both would make "delete
//! exactly what we marked" unprovable.

use std::time::Duration;

use aex_wire::ids::{ContentHash, WorkspaceId};

/// How long a presigned URL and its grant both live (OD-17).
#[allow(
    clippy::duration_suboptimal_units,
    reason = "`Duration::from_mins` is not const-stable on the pinned toolchain"
)]
pub const PRESIGN_EXPIRY: Duration = Duration::from_secs(300);

/// The largest `s3:signatureAge` the bucket policy admits, in milliseconds.
///
/// OD-17 pins this to the grant lifetime rather than the six minutes plan 05
/// §5.5 proposed for clock skew: a signature that outlives the grant that
/// authorised it is a bearer credential nobody is tracking.
pub const MAX_SIGNATURE_AGE_MILLIS: u64 = 300_000;

/// The largest single ranged read (`s3.single_get`).
pub const MAX_RANGE_BYTES: u64 = 5 * 1024 * 1024 * 1024 * 1024;

/// The metadata key carrying the owning workspace.
pub const METADATA_WORKSPACE: &str = "aex-workspace-id";

/// The metadata key carrying the customer-visible body digest.
pub const METADATA_DIGEST: &str = "aex-digest-sha256";

/// The metadata key carrying the declared plaintext size.
pub const METADATA_PLAINTEXT_BYTES: &str = "aex-plaintext-bytes";

/// A workspace-scoped, content-addressed object key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectKey(String);

impl ObjectKey {
    /// Composes `{workspace_id}/{sha256[0..2]}/{sha256[2..4]}/{sha256}`.
    ///
    /// The workspace prefix confines physical reuse to one encryption domain and
    /// makes a workspace-scoped bucket policy, inventory filter and lifecycle
    /// rule expressible. The two two-character levels keep S3 key partitioning
    /// healthy.
    #[must_use]
    pub fn new(workspace: WorkspaceId, digest: &ContentHash) -> Self {
        let hex = hex::encode(digest.as_bytes());
        Self(format!("{workspace}/{}/{}/{hex}", &hex[0..2], &hex[2..4]))
    }

    /// The key as S3 sees it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The prefix every object of one workspace shares.
    #[must_use]
    pub fn workspace_prefix(workspace: WorkspaceId) -> String {
        format!("{workspace}/")
    }
}

impl std::fmt::Display for ObjectKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The base64 spelling S3 wants for a SHA-256 checksum.
#[must_use]
pub fn checksum_base64(digest: &ContentHash) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(digest.as_bytes())
}

/// The base64 spelling S3 wants for an SSE-KMS encryption context.
#[must_use]
pub fn encryption_context_base64(canonical_json: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(canonical_json)
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{ContentHash, PrefixedId, Uuid7, WorkspaceId};

    use super::{MAX_SIGNATURE_AGE_MILLIS, ObjectKey, PRESIGN_EXPIRY, checksum_base64};

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    #[test]
    fn a_key_is_workspace_scoped_and_fans_out_twice() {
        let digest = ContentHash::from_bytes([0xab; 32]);
        let key = ObjectKey::new(workspace(1), &digest);
        let hex = hex::encode(digest.as_bytes());
        assert_eq!(
            key.as_str(),
            format!("{}/ab/ab/{hex}", workspace(1)),
            "the key is a pure function of the workspace and the digest"
        );
    }

    #[test]
    fn identical_bytes_in_two_workspaces_never_share_an_object() {
        let digest = ContentHash::from_bytes([1; 32]);
        assert_ne!(
            ObjectKey::new(workspace(1), &digest),
            ObjectKey::new(workspace(2), &digest),
            "cross-tenant physical reuse is exactly the defect this key fixes"
        );
    }

    #[test]
    fn a_signature_never_outlives_the_grant_that_authorised_it() {
        let expiry_millis = u64::try_from(PRESIGN_EXPIRY.as_millis()).expect("300 seconds");
        assert_eq!(
            expiry_millis, MAX_SIGNATURE_AGE_MILLIS,
            "OD-17 pins the two together so a leaked signature cannot outlive its grant"
        );
    }

    #[test]
    fn a_checksum_is_the_raw_digest_in_base64_and_never_its_hex_spelling() {
        let digest = ContentHash::from_bytes([0; 32]);
        assert_eq!(checksum_base64(&digest), "A".repeat(43) + "=");
    }
}
