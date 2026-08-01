//! `regional-content` row codecs.
//!
//! Two properties are load-bearing here. A body item and a descriptor item are
//! **separate rows**, so a descriptor read, a pin write or a mark scan never
//! pays for — or gains sight of — the ciphertext. And a download grant row
//! stores a *reference* to a body plus a pin, never a copy of the bytes (D-11);
//! the type simply has nowhere to put a body, so the amplification the previous
//! implementation had cannot be reintroduced by accident.

use aex_session_dynamodb::attr::{
    CodecError, Item, ItemBuilder, PK, Row, SK, b, n, n_i64, s, stamp,
};
use aex_session_dynamodb::component::KeyError;
use aex_session_dynamodb::measure;
use aex_wire::ids::{ContentHash, MeasurementId, OrganizationId, WorkspaceId};
use aex_wire::types::Timestamp;

use crate::keys;
use crate::wire_pending::{Blake3Digest, DigestError, PinOwner, SealedBytes, body_hex};

/// The `itemType` of a body descriptor.
pub const CONTENT_DESCRIPTOR: &str = "content_descriptor";
/// The `itemType` of an inline ciphertext body.
pub const CONTENT_BODY: &str = "content_body";
/// The `itemType` of a pin.
pub const CONTENT_PIN: &str = "content_pin";
/// The `itemType` of a download grant.
pub const DOWNLOAD_GRANT: &str = "download_grant";
/// The `itemType` of a root descriptor.
pub const ROOT_DESCRIPTOR: &str = "root_descriptor";
/// The `itemType` of a Merkle tree page.
pub const TREE_PAGE: &str = "tree_page";
/// The `itemType` of a garbage-collection epoch.
pub const GC_EPOCH: &str = "gc_epoch";
/// The `itemType` of a garbage-collection candidate.
pub const GC_CANDIDATE: &str = "gc_candidate";

/// Why a row could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A key component was unusable.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// A digest was not the family its position requires.
    #[error(transparent)]
    Digest(#[from] DigestError),
    /// The encoded row exceeded its ceiling.
    #[error("the row measures {measured} bytes; the ceiling is {ceiling}")]
    TooLarge {
        /// The measured encoded size.
        measured: usize,
        /// The ceiling that was exceeded.
        ceiling: usize,
    },
}

/// Where the body of a committed descriptor lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectLocation {
    /// The workspace-scoped object key.
    pub key: String,
    /// The `ETag` a fenced delete conditions on.
    pub etag: String,
    /// The checksum S3 stores: base64, and `COMPOSITE` for a multipart object.
    pub checksum_sha256: String,
    /// The full-object CRC64NVME checksum, base64.
    pub checksum_crc64_nvme: String,
    /// How many parts the multipart upload had, when it had any.
    pub part_count: Option<u64>,
    /// The content CMK the object is sealed under.
    pub kms_key_id: String,
}

/// One body descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDescriptor {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The owning organization.
    pub organization: OrganizationId,
    /// The customer-visible SHA-256 body digest.
    pub digest: ContentHash,
    /// The plaintext size.
    pub size_bytes: u64,
    /// The media type.
    pub media_type: String,
    /// Where the body lives.
    pub placement: measure::Placement,
    /// Whether the body has been committed.
    pub state: String,
    /// The epoch the body was last marked reachable in.
    pub gc_epoch: u64,
    /// When the descriptor was staged.
    pub created_at: Timestamp,
    /// When a deep verify last confirmed the bytes.
    pub verified_at: Option<Timestamp>,
    /// The object location, when the placement is the object store.
    pub object: Option<ObjectLocation>,
}

/// Encodes one body descriptor, including its garbage-collection scan
/// attributes.
///
/// # Errors
///
/// [`EncodeError`] when a key component is unusable or the row is over the
/// application ceiling.
pub fn encode_descriptor(descriptor: &ContentDescriptor) -> Result<Item, EncodeError> {
    let key = keys::descriptor(descriptor.workspace, &descriptor.digest);
    let digest_hex = body_hex(&descriptor.digest);
    let bucket = keys::bucket_of(&digest_hex)?;
    let mut builder = ItemBuilder::new(CONTENT_DESCRIPTOR)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(descriptor.workspace.to_string()))
        .set("organizationId", s(descriptor.organization.to_string()))
        .set("digestSha256", s(descriptor.digest.to_wire()))
        .set("sizeBytes", n(descriptor.size_bytes))
        .set("mediaType", s(descriptor.media_type.clone()))
        .set("placement", s(descriptor.placement.as_str()))
        .set("state", s(descriptor.state.clone()))
        .set("gcEpoch", n(descriptor.gc_epoch))
        .set("createdAt", stamp(descriptor.created_at))
        .set_opt("verifiedAt", descriptor.verified_at.map(stamp))
        .set(
            keys::GC_PK,
            s(keys::gc_scan_partition(descriptor.workspace, bucket)),
        )
        .set(
            keys::GC_SK,
            s(keys::gc_scan_sort(descriptor.created_at, &digest_hex)?),
        );
    if let Some(object) = &descriptor.object {
        builder = builder
            .set("objectKey", s(object.key.clone()))
            .set("objectEtag", s(object.etag.clone()))
            .set("objectChecksumSha256", s(object.checksum_sha256.clone()))
            .set(
                "objectChecksumCrc64Nvme",
                s(object.checksum_crc64_nvme.clone()),
            )
            .set_opt("objectPartCount", object.part_count.map(n))
            .set("kmsKeyId", s(object.kms_key_id.clone()));
    }
    let item = builder.build();
    measured(item, measure::APPLICATION_ITEM_CEILING)
}

/// Decodes one body descriptor and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, or
/// a row from another tenant.
pub fn decode_descriptor(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<ContentDescriptor, CodecError> {
    let row = Row::bind(item, CONTENT_DESCRIPTOR)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let placement = match row.enumerated("placement", keys::PLACEMENTS)? {
        "inline" => measure::Placement::Inline,
        _ => measure::Placement::ObjectStore,
    };
    let object = if placement == measure::Placement::ObjectStore {
        Some(ObjectLocation {
            key: row.string("objectKey")?.to_owned(),
            etag: row.string("objectEtag")?.to_owned(),
            checksum_sha256: row.string("objectChecksumSha256")?.to_owned(),
            checksum_crc64_nvme: row.string("objectChecksumCrc64Nvme")?.to_owned(),
            part_count: row.opt_u64("objectPartCount")?,
            kms_key_id: row.string("kmsKeyId")?.to_owned(),
        })
    } else {
        None
    };
    Ok(ContentDescriptor {
        workspace: asserted,
        organization: row.id::<OrganizationId>("organizationId")?,
        digest: content_hash(&row, "digestSha256")?,
        size_bytes: row.u64("sizeBytes")?,
        media_type: row.string("mediaType")?.to_owned(),
        placement,
        state: row.enumerated("state", keys::DESCRIPTOR_STATES)?.to_owned(),
        gc_epoch: row.u64("gcEpoch")?,
        created_at: row.timestamp("createdAt")?,
        verified_at: row.opt_timestamp("verifiedAt")?,
        object,
    })
}

/// Encodes one inline ciphertext body.
///
/// The body carries **no** garbage-collection index attribute: the descriptor is
/// what a mark scan enumerates, and duplicating the body into the index would
/// put ciphertext one projection edit away from a scan result.
///
/// # Errors
///
/// [`EncodeError::TooLarge`] when the sealed body exceeds the application
/// ceiling.
pub fn encode_inline_body(
    workspace: WorkspaceId,
    digest: &ContentHash,
    sealed: &SealedBytes,
) -> Result<Item, EncodeError> {
    let key = keys::inline_body(workspace, digest);
    let item = ItemBuilder::new(CONTENT_BODY)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(workspace.to_string()))
        .set("ciphertext", b(sealed.ciphertext.clone()))
        .set("encContextDigest", s(sealed.enc_context_digest.clone()))
        .build();
    measured(item, measure::APPLICATION_ITEM_CEILING)
}

/// Decodes one inline ciphertext body.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_inline_body(item: &Item, asserted: WorkspaceId) -> Result<SealedBytes, CodecError> {
    let row = Row::bind(item, CONTENT_BODY)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(SealedBytes {
        ciphertext: row.bytes("ciphertext")?.to_vec(),
        enc_context_digest: row.string("encContextDigest")?.to_owned(),
    })
}

/// One pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPin {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Who holds it.
    pub owner: PinOwner,
    /// When it was taken.
    pub created_at: Timestamp,
}

/// Encodes one pin item body, without its key.
///
/// The key is chosen by the caller — [`keys::pin`] for a loose body,
/// [`keys::root_pin`] for a root — because that choice is the D-10 decision and
/// belongs at the call site rather than inside the codec.
#[must_use]
pub fn pin_attributes(pin: &ContentPin) -> Item {
    ItemBuilder::new(CONTENT_PIN)
        .set("pinKind", s(pin.owner.kind()))
        .set("pinId", s(pin.owner.id()))
        .set("workspaceId", s(pin.workspace.to_string()))
        .set("createdAt", stamp(pin.created_at))
        .build()
}

/// Encodes the pin a download grant holds.
///
/// This is the only pin that expires, and the expiry is stored twice on purpose:
/// `expiresAt` is what the sweeper reads, `expiresAtEpochSeconds` is only how the
/// row is eventually reclaimed. TTL is never the fence (D-24).
#[must_use]
pub fn grant_pin_attributes(
    workspace: WorkspaceId,
    token_sha256_hex: &str,
    created_at: Timestamp,
    expires_at: Timestamp,
) -> Item {
    ItemBuilder::new(CONTENT_PIN)
        .set("pinKind", s(GRANT_PIN_KIND))
        .set("pinId", s(token_sha256_hex.to_owned()))
        .set("workspaceId", s(workspace.to_string()))
        .set("createdAt", stamp(created_at))
        .set("expiresAt", stamp(expires_at))
        .set(
            "expiresAtEpochSeconds",
            n_i64(ttl_epoch_seconds(expires_at)),
        )
        .build()
}

/// The pin kind a download grant's own pin declares.
///
/// It sits outside [`keys::PIN_KINDS`] because a grant pin is addressed by
/// `GRANT#{token}` rather than by `PIN#{kind}#{id}`, and because it is the only
/// pin kind that ever carries an expiry.
pub const GRANT_PIN_KIND: &str = "grant";

/// The pin kinds a stored `content_pin` row may declare.
pub const STORED_PIN_KINDS: &[&str] = &[
    "session",
    "registry",
    "operation",
    "export",
    "cursor",
    "gc",
    "message",
    GRANT_PIN_KIND,
];

/// One download grant.
///
/// There is deliberately no body field. A small-body grant stores a content
/// reference and a pin; redemption reads the pinned inline body (D-11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadGrant {
    /// The token digest. The token itself is never stored.
    pub token_sha256: String,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The body the grant reads.
    pub digest: ContentHash,
    /// The first authorised byte.
    pub range_start: u64,
    /// One past the last authorised byte.
    pub range_end_exclusive: u64,
    /// The authorised transfer size.
    pub authorized_bytes: u64,
    /// The transfer measurement.
    pub measurement: MeasurementId,
    /// The media type redemption serves.
    pub media_type: String,
    /// When the grant stops being redeemable.
    pub expires_at: Timestamp,
}

/// Encodes one download grant.
///
/// # Errors
///
/// [`EncodeError`] when the token digest could not enter a key.
pub fn encode_grant(grant: &DownloadGrant) -> Result<Item, EncodeError> {
    let key = keys::grant(&grant.token_sha256)?;
    Ok(ItemBuilder::new(DOWNLOAD_GRANT)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(grant.workspace.to_string()))
        .set("contentDigest", s(grant.digest.to_wire()))
        .set("rangeStart", n(grant.range_start))
        .set("rangeEndExclusive", n(grant.range_end_exclusive))
        .set("authorizedBytes", n(grant.authorized_bytes))
        .set("measurementId", s(grant.measurement.to_string()))
        .set("mediaType", s(grant.media_type.clone()))
        .set("expiresAt", stamp(grant.expires_at))
        .set(
            "expiresAtEpochSeconds",
            n_i64(ttl_epoch_seconds(grant.expires_at)),
        )
        .build())
}

/// Decodes one download grant.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_grant(item: &Item) -> Result<DownloadGrant, CodecError> {
    let row = Row::bind(item, DOWNLOAD_GRANT)?;
    Ok(DownloadGrant {
        token_sha256: row
            .string(PK)?
            .strip_prefix("GRANT#")
            .unwrap_or_default()
            .to_owned(),
        workspace: row.id::<WorkspaceId>("workspaceId")?,
        digest: content_hash(&row, "contentDigest")?,
        range_start: row.u64("rangeStart")?,
        range_end_exclusive: row.u64("rangeEndExclusive")?,
        authorized_bytes: row.u64("authorizedBytes")?,
        measurement: row.id::<MeasurementId>("measurementId")?,
        media_type: row.string("mediaType")?.to_owned(),
        expires_at: row.timestamp("expiresAt")?,
    })
}

/// One root descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootDescriptor {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The root digest.
    pub root: Blake3Digest,
    /// The page the root points at.
    pub tree_page: Blake3Digest,
    /// The logical size of everything under the root.
    pub logical_bytes: u64,
    /// How many entries the root names.
    pub entry_count: u64,
    /// When the root was written.
    pub created_at: Timestamp,
}

/// Encodes one root descriptor.
#[must_use]
pub fn encode_root(root: &RootDescriptor) -> Item {
    let key = keys::root_descriptor(root.workspace, root.root);
    ItemBuilder::new(ROOT_DESCRIPTOR)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(root.workspace.to_string()))
        .set("rootDigest", s(root.root.to_wire()))
        .set("treePageDigest", s(root.tree_page.to_wire()))
        .set("logicalBytes", n(root.logical_bytes))
        .set("entryCount", n(root.entry_count))
        .set("createdAt", stamp(root.created_at))
        .build()
}

/// Decodes one root descriptor.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_root(item: &Item, asserted: WorkspaceId) -> Result<RootDescriptor, CodecError> {
    let row = Row::bind(item, ROOT_DESCRIPTOR)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(RootDescriptor {
        workspace: asserted,
        root: blake3(&row, "rootDigest")?,
        tree_page: blake3(&row, "treePageDigest")?,
        logical_bytes: row.u64("logicalBytes")?,
        entry_count: row.u64("entryCount")?,
        created_at: row.timestamp("createdAt")?,
    })
}

/// One Merkle tree page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePage {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The page digest.
    pub page: Blake3Digest,
    /// How far above the leaves the page sits.
    pub level: u64,
    /// How many entries it carries.
    pub entry_count: u64,
    /// The sealed page.
    pub sealed: SealedBytes,
    /// When the page was written.
    pub created_at: Timestamp,
}

/// Encodes one Merkle tree page.
///
/// # Errors
///
/// [`EncodeError::TooLarge`] above the 192 KiB page target, which is checked
/// here rather than left to a provider `400`.
pub fn encode_tree_page(page: &TreePage) -> Result<Item, EncodeError> {
    let key = keys::tree_page(page.workspace, page.page);
    let digest_hex = page.page.to_hex();
    let bucket = keys::bucket_of(&digest_hex)?;
    let item = ItemBuilder::new(TREE_PAGE)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(page.workspace.to_string()))
        .set("pageDigest", s(page.page.to_wire()))
        .set("level", n(page.level))
        .set("entryCount", n(page.entry_count))
        .set("ciphertext", b(page.sealed.ciphertext.clone()))
        .set(
            "encContextDigest",
            s(page.sealed.enc_context_digest.clone()),
        )
        .set("createdAt", stamp(page.created_at))
        .set(
            keys::GC_PK,
            s(keys::gc_scan_partition(page.workspace, bucket)),
        )
        .set(
            keys::GC_SK,
            s(keys::gc_scan_sort(page.created_at, &digest_hex)?),
        )
        .build();
    measured(item, measure::PAGE_TARGET_BYTES)
}

/// Decodes one Merkle tree page.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_tree_page(item: &Item, asserted: WorkspaceId) -> Result<TreePage, CodecError> {
    let row = Row::bind(item, TREE_PAGE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(TreePage {
        workspace: asserted,
        page: blake3(&row, "pageDigest")?,
        level: row.u64("level")?,
        entry_count: row.u64("entryCount")?,
        sealed: SealedBytes {
            ciphertext: row.bytes("ciphertext")?.to_vec(),
            enc_context_digest: row.string("encContextDigest")?.to_owned(),
        },
        created_at: row.timestamp("createdAt")?,
    })
}

/// The workspace's garbage-collection epoch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcEpoch {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The current epoch.
    pub epoch: u64,
    /// What the collector is doing.
    pub state: String,
    /// When the current mark started.
    pub mark_started_at: Option<Timestamp>,
    /// How far the mark has walked the bucket space.
    pub mark_bucket_cursor: Option<u64>,
    /// Where the sweep resumes.
    pub sweep_cursor: Option<String>,
    /// The optimistic revision.
    pub revision: u64,
}

/// Encodes the garbage-collection epoch.
#[must_use]
pub fn encode_gc_epoch(epoch: &GcEpoch) -> Item {
    let key = keys::gc_epoch(epoch.workspace);
    ItemBuilder::new(GC_EPOCH)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(epoch.workspace.to_string()))
        .set("epoch", n(epoch.epoch))
        .set("state", s(epoch.state.clone()))
        .set_opt("markStartedAt", epoch.mark_started_at.map(stamp))
        .set_opt("markBucketCursor", epoch.mark_bucket_cursor.map(n))
        .set_opt(
            "sweepCursor",
            epoch.sweep_cursor.as_ref().map(|cursor| s(cursor.clone())),
        )
        .set("revision", n(epoch.revision))
        .build()
}

/// Decodes the garbage-collection epoch.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_gc_epoch(item: &Item, asserted: WorkspaceId) -> Result<GcEpoch, CodecError> {
    let row = Row::bind(item, GC_EPOCH)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(GcEpoch {
        workspace: asserted,
        epoch: row.u64("epoch")?,
        state: row.enumerated("state", keys::GC_STATES)?.to_owned(),
        mark_started_at: row.opt_timestamp("markStartedAt")?,
        mark_bucket_cursor: row.opt_u64("markBucketCursor")?,
        sweep_cursor: row.opt_string("sweepCursor")?.map(str::to_owned),
        revision: row.u64("revision")?,
    })
}

/// One garbage-collection candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcCandidate {
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The body proposed for deletion.
    pub digest: ContentHash,
    /// The epoch it was staged in.
    pub epoch: u64,
    /// When it was staged.
    pub staged_at: Timestamp,
    /// The object key, when the body is in the object store.
    pub object_key: Option<String>,
    /// The `ETag` the fenced delete conditions on.
    pub object_etag: Option<String>,
    /// The body size.
    pub size_bytes: u64,
    /// How many sweep attempts have been made.
    pub attempt_count: u64,
}

impl GcCandidate {
    /// The instant before which a sweep must refuse to act.
    ///
    /// # Errors
    ///
    /// [`aex_wire::types::ValueError`] only if the hold pushes the instant out
    /// of representable range.
    pub fn not_before(&self) -> Result<Timestamp, aex_wire::types::ValueError> {
        Timestamp::from_unix_millis(
            self.staged_at.unix_millis() + keys::CANDIDATE_HOLD_SECONDS * 1_000,
        )
    }
}

/// Encodes one garbage-collection candidate.
///
/// # Errors
///
/// [`EncodeError`] when a key component is unusable or the hold instant is out
/// of range.
pub fn encode_gc_candidate(candidate: &GcCandidate) -> Result<Item, EncodeError> {
    let key = keys::gc_candidate(candidate.workspace, &candidate.digest);
    let digest_hex = body_hex(&candidate.digest);
    let bucket = keys::bucket_of(&digest_hex)?;
    let not_before = candidate.not_before().map_err(|_| EncodeError::TooLarge {
        measured: 0,
        ceiling: 0,
    })?;
    Ok(ItemBuilder::new(GC_CANDIDATE)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(candidate.workspace.to_string()))
        .set("epoch", n(candidate.epoch))
        .set("stagedAt", stamp(candidate.staged_at))
        .set("notBefore", stamp(not_before))
        .set("digestSha256", s(candidate.digest.to_wire()))
        .set_opt(
            "objectKey",
            candidate.object_key.as_ref().map(|key| s(key.clone())),
        )
        .set_opt(
            "objectEtag",
            candidate.object_etag.as_ref().map(|etag| s(etag.clone())),
        )
        .set("sizeBytes", n(candidate.size_bytes))
        .set("attemptCount", n(candidate.attempt_count))
        .set(
            keys::GC_PK,
            s(keys::gc_scan_partition(candidate.workspace, bucket)),
        )
        .set(
            keys::GC_SK,
            s(keys::gc_scan_sort(candidate.staged_at, &digest_hex)?),
        )
        .build())
}

/// Decodes one garbage-collection candidate.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_gc_candidate(item: &Item, asserted: WorkspaceId) -> Result<GcCandidate, CodecError> {
    let row = Row::bind(item, GC_CANDIDATE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(GcCandidate {
        workspace: asserted,
        digest: content_hash(&row, "digestSha256")?,
        epoch: row.u64("epoch")?,
        staged_at: row.timestamp("stagedAt")?,
        object_key: row.opt_string("objectKey")?.map(str::to_owned),
        object_etag: row.opt_string("objectEtag")?.map(str::to_owned),
        size_bytes: row.u64("sizeBytes")?,
        attempt_count: row.u64("attemptCount")?,
    })
}

/// The TTL value a row expiring at `expires_at` carries.
///
/// The grace is deliberate: a redemption arriving in the last second must find
/// the row and be refused by the explicit `expiresAt` check, not by an absence
/// it cannot tell apart from a forged token.
#[must_use]
pub fn ttl_epoch_seconds(expires_at: Timestamp) -> i64 {
    expires_at.unix_millis().div_euclid(1_000) + keys::GRANT_TTL_GRACE_SECONDS
}

fn measured(item: Item, ceiling: usize) -> Result<Item, EncodeError> {
    let size = measure::item_bytes(&item);
    if size > ceiling {
        return Err(EncodeError::TooLarge {
            measured: size,
            ceiling,
        });
    }
    Ok(item)
}

fn content_hash(row: &Row<'_>, attribute: &'static str) -> Result<ContentHash, CodecError> {
    let text = row.string(attribute)?;
    ContentHash::parse(text).map_err(|error| CodecError::Malformed {
        item_type: CONTENT_DESCRIPTOR,
        attribute,
        reason: error.to_string(),
    })
}

fn blake3(row: &Row<'_>, attribute: &'static str) -> Result<Blake3Digest, CodecError> {
    let text = row.string(attribute)?;
    Blake3Digest::parse(text).map_err(|error| CodecError::Malformed {
        item_type: TREE_PAGE,
        attribute,
        reason: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use aex_session_dynamodb::attr::CodecError;
    use aex_session_dynamodb::measure;
    use aex_wire::ids::{
        ContentHash, MeasurementId, OrganizationId, PrefixedId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::Timestamp;

    use super::{
        ContentDescriptor, DownloadGrant, EncodeError, GcCandidate, ObjectLocation, TreePage,
        decode_descriptor, decode_grant, decode_tree_page, encode_descriptor, encode_gc_candidate,
        encode_grant, encode_inline_body, encode_tree_page,
    };
    use crate::keys;
    use crate::wire_pending::{Blake3Digest, SealedBytes};

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    fn sealed(bytes: usize) -> SealedBytes {
        SealedBytes {
            ciphertext: vec![7u8; bytes],
            enc_context_digest: "a".repeat(64),
        }
    }

    fn descriptor() -> ContentDescriptor {
        ContentDescriptor {
            workspace: workspace(1),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            digest: ContentHash::from_bytes([0xab; 32]),
            size_bytes: 4_096,
            media_type: "text/plain".to_owned(),
            placement: measure::Placement::Inline,
            state: "staged".to_owned(),
            gc_epoch: 3,
            created_at: now(),
            verified_at: None,
            object: None,
        }
    }

    fn grant() -> DownloadGrant {
        DownloadGrant {
            token_sha256: "b".repeat(64),
            workspace: workspace(1),
            digest: ContentHash::from_bytes([0xab; 32]),
            range_start: 0,
            range_end_exclusive: 4_096,
            authorized_bytes: 4_096,
            measurement: MeasurementId::from_uuid7(Uuid7::compose(1, [9; 10])),
            media_type: "text/plain".to_owned(),
            expires_at: now(),
        }
    }

    #[test]
    fn a_descriptor_round_trips_exactly() {
        let original = descriptor();
        let encoded = encode_descriptor(&original).expect("encodes");
        assert_eq!(
            decode_descriptor(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn an_object_backed_descriptor_round_trips_with_both_checksums() {
        let mut original = descriptor();
        original.placement = measure::Placement::ObjectStore;
        original.state = "committed".to_owned();
        original.object = Some(ObjectLocation {
            key: "ws_1/ab/cd/abcd".to_owned(),
            etag: "\"etag\"".to_owned(),
            checksum_sha256: "Y2hlY2tzdW0=".to_owned(),
            checksum_crc64_nvme: "Y3JjNjQ=".to_owned(),
            part_count: Some(3),
            kms_key_id: "arn:aws:kms:eu-west-1:1:key/abc".to_owned(),
        });
        let encoded = encode_descriptor(&original).expect("encodes");
        assert_eq!(
            decode_descriptor(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn a_descriptor_from_another_tenant_is_rejected_after_read() {
        let encoded = encode_descriptor(&descriptor()).expect("encodes");
        let error = decode_descriptor(&encoded, workspace(9)).expect_err("another tenant");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }

    #[test]
    fn the_body_digest_is_sha256_and_the_page_digest_is_blake3() {
        let encoded = encode_descriptor(&descriptor()).expect("encodes");
        let stored = encoded
            .get("digestSha256")
            .and_then(|value| value.as_s().ok())
            .expect("a digest");
        assert!(stored.starts_with("sha256:"), "{stored}");

        let page = TreePage {
            workspace: workspace(1),
            page: Blake3Digest::of(b"page"),
            level: 0,
            entry_count: 2,
            sealed: sealed(16),
            created_at: now(),
        };
        let encoded = encode_tree_page(&page).expect("encodes");
        let stored = encoded
            .get("pageDigest")
            .and_then(|value| value.as_s().ok())
            .expect("a digest");
        assert!(stored.starts_with("b3:"), "{stored}");
        assert_eq!(
            decode_tree_page(&encoded, workspace(1)).expect("decodes"),
            page
        );
    }

    #[test]
    fn a_page_digest_never_decodes_as_a_body_digest() {
        assert!(Blake3Digest::parse(&ContentHash::from_bytes([1; 32]).to_wire()).is_err());
        assert!(ContentHash::parse(&Blake3Digest::from_bytes([1; 32]).to_wire()).is_err());
    }

    #[test]
    fn a_download_grant_stores_a_reference_and_never_a_body() {
        let encoded = encode_grant(&grant()).expect("encodes");
        for forbidden in ["ciphertext", "body", "bodyInline", "content"] {
            assert!(
                !encoded.contains_key(forbidden),
                "a grant row must never carry `{forbidden}`"
            );
        }
        assert_eq!(decode_grant(&encoded).expect("decodes"), grant());
    }

    #[test]
    fn an_inline_body_carries_no_garbage_collection_index_attribute() {
        let encoded =
            encode_inline_body(workspace(1), &ContentHash::from_bytes([1; 32]), &sealed(32))
                .expect("encodes");
        assert!(!encoded.contains_key(keys::GC_PK));
        assert!(!encoded.contains_key(keys::GC_SK));
    }

    #[test]
    fn a_tree_page_over_the_page_target_is_refused_with_its_measurement() {
        let page = TreePage {
            workspace: workspace(1),
            page: Blake3Digest::of(b"big"),
            level: 0,
            entry_count: 1,
            sealed: sealed(measure::PAGE_TARGET_BYTES + 1),
            created_at: now(),
        };
        let error = encode_tree_page(&page).expect_err("over the page target");
        assert!(matches!(error, EncodeError::TooLarge { .. }), "{error}");
    }

    #[test]
    fn a_candidate_cannot_be_swept_before_its_hold_elapses() {
        let candidate = GcCandidate {
            workspace: workspace(1),
            digest: ContentHash::from_bytes([2; 32]),
            epoch: 4,
            staged_at: now(),
            object_key: None,
            object_etag: None,
            size_bytes: 10,
            attempt_count: 0,
        };
        let encoded = encode_gc_candidate(&candidate).expect("encodes");
        let not_before = encoded
            .get("notBefore")
            .and_then(|value| value.as_s().ok())
            .expect("a hold");
        assert!(
            Timestamp::parse(not_before).expect("a timestamp") > now(),
            "a freshly staged candidate is never immediately sweepable"
        );
    }
}
