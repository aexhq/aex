//! The `regional-content` key templates and closed vocabularies.
//!
//! Every content partition is **workspace scoped**. The system this replaces
//! addressed a body as `bodies/{contentId}/{xx}/{digest}` with no workspace
//! component, so two tenants that happened to store identical bytes shared one
//! physical object and one physical row. That is a cross-tenant equality side
//! channel and a shared deletion fate; scoping the partition to the workspace
//! confines physical reuse to a single encryption domain (D-15). This is a fix,
//! not a port.

use aex_wire::ids::{ContentHash, WorkspaceId};
use aex_wire::types::Timestamp;

use aex_session_dynamodb::component::{Component, KeyError, bucket3, gc_bucket};

use crate::wire_pending::{Blake3Digest, PinOwner, body_hex};

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

impl Key {
    fn new(pk: String, sk: String) -> Self {
        Self { pk, sk }
    }
}

/// Every `itemType` this table may hold, as declared in
/// `migrations/regional/tables/regional-content.json`.
pub const ITEM_TYPES: &[&str] = &[
    "content_descriptor",
    "content_body",
    "content_pin",
    "download_grant",
    "root_descriptor",
    "tree_page",
    "gc_epoch",
    "gc_candidate",
];

/// The closed `pin_kind` vocabulary.
pub const PIN_KINDS: &[&str] = &[
    "session",
    "registry",
    "operation",
    "export",
    "cursor",
    "gc",
    "message",
];

/// The closed descriptor placement vocabulary.
pub const PLACEMENTS: &[&str] = &["inline", "s3"];

/// The closed descriptor state vocabulary.
pub const DESCRIPTOR_STATES: &[&str] = &["staged", "committed"];

/// The closed garbage-collection epoch state vocabulary.
pub const GC_STATES: &[&str] = &["idle", "marking", "sweeping"];

/// How many buckets the garbage-collection scan spreads over
/// (`content.gc_buckets`).
pub const GC_BUCKETS: u16 = 256;

/// The garbage-collection scan index name.
pub const GC_INDEX: &str = "gsi_gc";

/// The garbage-collection index partition key attribute.
pub const GC_PK: &str = "gcScanPk";

/// The garbage-collection index sort key attribute.
pub const GC_SK: &str = "gcScanSk";

/// Exactly the attributes `gsi_gc` projects.
///
/// The list is asserted against the generation definition, so a codec cannot
/// start writing a ciphertext attribute that the index would then carry into a
/// mark scan.
pub const GC_PROJECTION: &[&str] = &[
    "workspaceId",
    "digestSha256",
    "pageDigest",
    "placement",
    "sizeBytes",
    "objectKey",
    "objectEtag",
    "gcEpoch",
    "notBefore",
    "state",
];

/// How long a grant row outlives its own expiry before TTL may reclaim it.
///
/// The grace exists so a redemption arriving at the last moment still finds the
/// row and is refused by the explicit `expiresAt` check rather than by an
/// absence it cannot distinguish from a bad token.
pub const GRANT_TTL_GRACE_SECONDS: i64 = 3_600;

/// How long a staged body is protected from sweeping.
pub const CANDIDATE_HOLD_SECONDS: i64 = 24 * 60 * 60;

/// `CONTENT#{workspace_id}#{sha256_hex}`.
#[must_use]
pub fn content_partition(workspace: WorkspaceId, digest: &ContentHash) -> String {
    format!("CONTENT#{workspace}#{}", body_hex(digest))
}

/// `ROOT#{workspace_id}#{root_b3_hex}`.
#[must_use]
pub fn root_partition(workspace: WorkspaceId, root: Blake3Digest) -> String {
    format!("ROOT#{workspace}#{}", root.to_hex())
}

/// The body descriptor.
#[must_use]
pub fn descriptor(workspace: WorkspaceId, digest: &ContentHash) -> Key {
    Key::new(content_partition(workspace, digest), "DESC".to_owned())
}

/// The inline ciphertext body.
///
/// The body lives in its own item so a descriptor read, a pin write or a
/// garbage-collection scan never pays for the body bytes.
#[must_use]
pub fn inline_body(workspace: WorkspaceId, digest: &ContentHash) -> Key {
    Key::new(content_partition(workspace, digest), "BODY".to_owned())
}

/// A direct pin on one loose body.
///
/// # Errors
///
/// [`KeyError`] when the pin identity could not enter a key.
pub fn pin(
    workspace: WorkspaceId,
    digest: &ContentHash,
    owner: &PinOwner,
) -> Result<Key, KeyError> {
    Ok(Key::new(
        content_partition(workspace, digest),
        pin_sort(owner)?,
    ))
}

/// A pin on one root, which is where a pin belongs whenever a root exists.
///
/// Pins live on roots, not on every body, so a session that binds a 10,000-file
/// tool bundle writes **one** pin item rather than 10,000 (D-10). Only loose
/// bodies — message content, tool results, journal overflow — carry a direct
/// [`pin`], and those are one pin per body by construction.
///
/// # Errors
///
/// As [`pin`].
pub fn root_pin(
    workspace: WorkspaceId,
    root: Blake3Digest,
    owner: &PinOwner,
) -> Result<Key, KeyError> {
    Ok(Key::new(root_partition(workspace, root), pin_sort(owner)?))
}

fn pin_sort(owner: &PinOwner) -> Result<String, KeyError> {
    let id = owner.id();
    let id = Component::parse(&id)?;
    Ok(format!("PIN#{}#{id}", owner.kind()))
}

/// The sort-key prefix every pin shares.
#[must_use]
pub const fn pin_prefix() -> &'static str {
    "PIN#"
}

/// The pin a download grant holds on the body it reads.
///
/// # Errors
///
/// [`KeyError`] when the token digest could not enter a key.
pub fn grant_pin(
    workspace: WorkspaceId,
    digest: &ContentHash,
    token_sha256_hex: &str,
) -> Result<Key, KeyError> {
    let token = Component::parse(token_sha256_hex)?;
    Ok(Key::new(
        content_partition(workspace, digest),
        format!("GRANT#{token}"),
    ))
}

/// The sort-key prefix every grant pin shares.
#[must_use]
pub const fn grant_pin_prefix() -> &'static str {
    "GRANT#"
}

/// The grant lookup a redemption reads.
///
/// # Errors
///
/// [`KeyError`] when the token digest could not enter a key.
pub fn grant(token_sha256_hex: &str) -> Result<Key, KeyError> {
    let token = Component::parse(token_sha256_hex)?;
    Ok(Key::new(format!("GRANT#{token}"), "STATE".to_owned()))
}

/// The root descriptor.
#[must_use]
pub fn root_descriptor(workspace: WorkspaceId, root: Blake3Digest) -> Key {
    Key::new(root_partition(workspace, root), "DESC".to_owned())
}

/// One Merkle tree page.
#[must_use]
pub fn tree_page(workspace: WorkspaceId, page: Blake3Digest) -> Key {
    Key::new(
        format!("TREE#{workspace}#{}", page.to_hex()),
        "PAGE".to_owned(),
    )
}

/// The workspace's garbage-collection epoch.
#[must_use]
pub fn gc_epoch(workspace: WorkspaceId) -> Key {
    Key::new(format!("GC#{workspace}"), "EPOCH".to_owned())
}

/// One garbage-collection candidate.
#[must_use]
pub fn gc_candidate(workspace: WorkspaceId, digest: &ContentHash) -> Key {
    Key::new(
        format!("GCCAND#{workspace}#{}", body_hex(digest)),
        "CANDIDATE".to_owned(),
    )
}

/// The garbage-collection scan partition for one bucket.
#[must_use]
pub fn gc_scan_partition(workspace: WorkspaceId, bucket: u16) -> String {
    format!("GCSCAN#{workspace}#{}", bucket3(bucket))
}

/// Which bucket a body digest scans in.
///
/// # Errors
///
/// [`KeyError`] when the hex is too short to take a bucket from.
pub fn bucket_of(digest_hex: &str) -> Result<u16, KeyError> {
    gc_bucket(digest_hex, GC_BUCKETS)
}

/// The garbage-collection scan sort key.
///
/// # Errors
///
/// [`KeyError`] when the digest could not enter a key.
pub fn gc_scan_sort(created_at: Timestamp, digest_hex: &str) -> Result<String, KeyError> {
    let digest = Component::parse(digest_hex)?;
    Ok(format!("{}#{digest}", created_at.to_wire()))
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{ContentHash, PrefixedId, SessionId, Uuid7, WorkspaceId};

    use super::{
        GC_BUCKETS, bucket_of, content_partition, descriptor, gc_candidate, gc_epoch,
        gc_scan_partition, grant, grant_pin, inline_body, pin, root_pin, tree_page,
    };
    use crate::wire_pending::{Blake3Digest, PinOwner};

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn digest(byte: u8) -> ContentHash {
        ContentHash::from_bytes([byte; 32])
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
    }

    #[test]
    fn every_content_partition_is_workspace_scoped() {
        let body = digest(0xab);
        let first = content_partition(workspace(1), &body);
        let second = content_partition(workspace(2), &body);
        assert_ne!(
            first, second,
            "identical bytes in two workspaces must never share a partition"
        );
        assert!(first.starts_with("CONTENT#wsp_"));
    }

    #[test]
    fn the_descriptor_the_body_and_a_pin_share_one_partition() {
        let body = digest(1);
        let workspace = workspace(1);
        let partition = content_partition(workspace, &body);
        assert_eq!(descriptor(workspace, &body).pk, partition);
        assert_eq!(inline_body(workspace, &body).pk, partition);
        assert_eq!(
            pin(workspace, &body, &PinOwner::Session(session()))
                .expect("a pin")
                .pk,
            partition
        );
    }

    #[test]
    fn a_binding_pins_its_root_rather_than_every_body_underneath_it() {
        let root = Blake3Digest::from_bytes([7; 32]);
        let key = root_pin(workspace(1), root, &PinOwner::Session(session())).expect("a pin");
        assert!(key.pk.starts_with("ROOT#wsp_"));
        assert!(key.sk.starts_with("PIN#session#ses_"));
    }

    #[test]
    fn a_registry_pin_identity_is_the_kind_and_the_name() {
        let key = root_pin(
            workspace(1),
            Blake3Digest::from_bytes([8; 32]),
            &PinOwner::Registry {
                kind: "tool".to_owned(),
                name: "search".to_owned(),
            },
        )
        .expect("a pin");
        assert_eq!(key.sk, "PIN#registry#tool:search");
    }

    #[test]
    fn a_pin_identity_carrying_the_separator_can_never_reach_a_key() {
        let error = root_pin(
            workspace(1),
            Blake3Digest::from_bytes([9; 32]),
            &PinOwner::Export("exp#evil".to_owned()),
        );
        assert!(error.is_err());
    }

    #[test]
    fn a_grant_is_addressed_by_the_token_digest_and_never_by_the_token() {
        let token = "e".repeat(64);
        assert_eq!(grant(&token).expect("a key").pk, format!("GRANT#{token}"));
        assert_eq!(
            grant_pin(workspace(1), &digest(2), &token)
                .expect("a key")
                .sk,
            format!("GRANT#{token}")
        );
        assert!(grant("tok#evil").is_err());
    }

    #[test]
    fn a_tree_page_a_gc_epoch_and_a_candidate_are_all_workspace_scoped() {
        let workspace = workspace(1);
        assert!(
            tree_page(workspace, Blake3Digest::from_bytes([1; 32]))
                .pk
                .starts_with("TREE#wsp_")
        );
        assert!(gc_epoch(workspace).pk.starts_with("GC#wsp_"));
        assert!(
            gc_candidate(workspace, &digest(1))
                .pk
                .starts_with("GCCAND#wsp_")
        );
        assert!(gc_scan_partition(workspace, 255).ends_with("#255"));
    }

    #[test]
    fn the_scan_bucket_comes_from_the_digest_and_stays_inside_its_range() {
        for byte in 0..=255u8 {
            let bucket = bucket_of(&hex::encode([byte; 32])).expect("a bucket");
            assert!(bucket < GC_BUCKETS);
        }
    }
}
