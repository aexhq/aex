//! `regional-registry` row codecs.
//!
//! The `ETag` is **never stored as an independent fact**: it is recomputed from
//! `(kind, revision, digest)` on the way out and compared with what the row
//! carries. A stored tag that disagrees with its own row is corruption, and a
//! reader that trusted the stored copy would hand a client a concurrency token
//! that no longer describes the value it names.

use aex_content_domain::identity::{RegistryKind, Revision};
use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, PK, Row, SK, n, n_i64, s, stamp};
use aex_session_dynamodb::component::KeyError;
use aex_session_dynamodb::replay::{Receipt, decode_receipt_row, encode_receipt_row};
use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, UploadId, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{
    RegistryPointer, RegistryRow, RegistryState, ValueDocument, etag_of,
};
use aex_workspace_domain::upload::{
    CompletionEvidence, PartPlan, PlannedPart, RegistrySelector, SubmittedPart, Upload, UploadState,
};

use crate::keys;

/// The `itemType` of a current pointer.
pub const REGISTRY_POINTER: &str = "registry_pointer";
/// The `itemType` of a staged upload.
pub const REGISTRY_UPLOAD: &str = "registry_upload";
/// The `itemType` of one spilled block of an upload's part arrays.
pub const REGISTRY_UPLOAD_PARTS: &str = "registry_upload_parts";
/// The `itemType` of an idempotency receipt.
pub const IDEMPOTENCY_RECEIPT: &str = "idempotency_receipt";
/// The `itemType` of one `(workspace, kind)` entry counter.
pub const REGISTRY_COUNT: &str = "registry_count";

/// The attribute a pointer's canonical value document is stored under.
pub const VALUE_DOC: &str = "valueDoc";

/// The attribute holding one `(workspace, kind)` entry count.
pub const COUNT: &str = "entryCount";

/// The collection columns, as a `ProjectionExpression`.
///
/// A listing reads exactly these and never `valueDoc`: a thousand-row page that
/// carried a thousand value documents would be the cost D-3 exists to avoid.
/// `name` is a `DynamoDB` reserved word, so it is aliased.
pub const ROW_PROJECTION: &str = "itemType, workspaceId, kind, #name, revision, etag, contentDigest, sizeBytes, #state, failureCode, createdAt, \
     updatedAt";

/// Stable private spelling of one current-pointer lifecycle state.
#[must_use]
pub const fn registry_state_str(state: RegistryState) -> &'static str {
    match state {
        RegistryState::Pending => "pending",
        RegistryState::Ready => "ready",
        RegistryState::Failed => "failed",
    }
}

fn registry_state_of(text: &str) -> Option<RegistryState> {
    match text {
        "pending" => Some(RegistryState::Pending),
        "ready" => Some(RegistryState::Ready),
        "failed" => Some(RegistryState::Failed),
        _ => None,
    }
}

/// How many parts one persisted block carries (E D-1).
///
/// The part arrays cost roughly 100 bytes per part — `{number}:{bytes}:{sha256}`
/// plus a manifest `ETag` — so a 10 000-part upload would be about a megabyte,
/// well over `DynamoDB`'s 400 KB item limit. Above this bound the arrays spill to
/// sibling `PARTS#{block:04}` items. A 10 000-part upload is at least 50 GB, so
/// the common case stays a single item.
pub const PARTS_PER_BLOCK: usize = 1_000;

/// One upload as the rows that carry it.
///
/// The head row is always written; `part_blocks` is empty for every upload of
/// [`PARTS_PER_BLOCK`] parts or fewer, which is every upload anyone will stage in
/// practice.
#[derive(Debug, Clone)]
pub struct UploadRows {
    /// The `UPLOAD#{id}` / `STATE` row.
    pub head: Item,
    /// The `UPLOAD#{id}` / `PARTS#{block:04}` rows, in block order.
    pub part_blocks: Vec<Item>,
}

/// Why a row could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A key component was unusable.
    #[error(transparent)]
    Key(#[from] KeyError),
}

/// The stable wire spelling of one upload state.
#[must_use]
pub const fn upload_state_str(state: UploadState) -> &'static str {
    match state {
        UploadState::Created => "created",
        UploadState::PartsGranted => "parts_granted",
        UploadState::Completing => "completing",
        UploadState::Ready => "ready",
        UploadState::Consumed => "consumed",
        UploadState::Aborted => "aborted",
        UploadState::Expired => "expired",
    }
}

/// Resolves one upload state. There is no alias table.
#[must_use]
pub fn upload_state_of(text: &str) -> Option<UploadState> {
    UploadState::ALL
        .into_iter()
        .find(|state| upload_state_str(*state) == text)
}

/// Encodes one current pointer.
///
/// # Errors
///
/// [`EncodeError`] when the name could not enter a key.
pub fn encode_pointer(pointer: &RegistryPointer) -> Result<Item, EncodeError> {
    let row = &pointer.row;
    let key = keys::pointer(row.workspace, row.kind, row.name.as_str())?;
    Ok(ItemBuilder::new(REGISTRY_POINTER)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(row.workspace.to_string()))
        .set("kind", s(row.kind.as_str()))
        .set("name", s(row.name.as_str().to_owned()))
        .set("revision", n(row.revision.0))
        .set("etag", s(row.etag.as_str().to_owned()))
        .set(VALUE_DOC, s(pointer.value_doc.as_str().to_owned()))
        .set("contentDigest", s(row.sha256.to_wire()))
        .set("sizeBytes", n(row.size_bytes))
        .set("state", s(registry_state_str(row.state)))
        .set_opt("failureCode", row.failure_code.clone().map(s))
        .set("createdAt", stamp(row.created_at))
        .set("updatedAt", stamp(row.updated_at))
        .build())
}

/// Decodes the collection columns of one pointer.
///
/// This is what a listing reads. It never touches `valueDoc`, so it works
/// against a projected item as well as a complete one.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, a
/// row from another tenant, or a stored `ETag` that the row's own
/// `(kind, revision, digest)` does not produce.
pub fn decode_row(item: &Item, asserted: WorkspaceId) -> Result<RegistryRow, CodecError> {
    let row = Row::bind(item, REGISTRY_POINTER)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let kind_text = row.enumerated("kind", keys::KINDS)?;
    let kind = RegistryKind::parse(kind_text).ok_or(CodecError::Malformed {
        item_type: REGISTRY_POINTER,
        attribute: "kind",
        reason: "outside the registry kind vocabulary".to_owned(),
    })?;
    let name = aex_wire::ids::ResourceName::parse(row.string("name")?).map_err(|error| {
        CodecError::Malformed {
            item_type: REGISTRY_POINTER,
            attribute: "name",
            reason: error.to_string(),
        }
    })?;
    let revision = Revision(row.u64("revision")?);
    let sha256 = content_hash(&row, "contentDigest")?;
    let stored = ETag::parse(row.string("etag")?).map_err(|error| CodecError::Malformed {
        item_type: REGISTRY_POINTER,
        attribute: "etag",
        reason: error.to_string(),
    })?;
    let state = registry_state_of(row.string("state")?).ok_or(CodecError::Malformed {
        item_type: REGISTRY_POINTER,
        attribute: "state",
        reason: "outside the current-pointer lifecycle vocabulary".to_owned(),
    })?;
    let failure_code = row.opt_string("failureCode")?.map(str::to_owned);
    if (state == RegistryState::Failed) != failure_code.is_some() {
        return Err(CodecError::Malformed {
            item_type: REGISTRY_POINTER,
            attribute: "failureCode",
            reason: "present exactly when state is failed".to_owned(),
        });
    }
    let recomputed = etag_of(kind, revision, &sha256);
    if stored != recomputed {
        return Err(CodecError::Malformed {
            item_type: REGISTRY_POINTER,
            attribute: "etag",
            reason: "the stored tag is not the tag this row's own kind, revision and digest \
                     produce"
                .to_owned(),
        });
    }
    Ok(RegistryRow {
        workspace: asserted,
        kind,
        name,
        revision,
        etag: stored,
        sha256,
        size_bytes: row.u64("sizeBytes")?,
        state,
        failure_code,
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// Decodes one complete pointer, re-checking its ownership, its `ETag` **and**
/// the digest of its own value document.
///
/// The digest check is the same fence as the `ETag` check one step earlier: the
/// row states `sha256`, and since D-4 that field means `SHA-256(JCS(valueDoc))`.
/// A row whose two halves disagree is corruption, and a reader that trusted the
/// stored digest would publish a value document under a tag that does not
/// describe it.
///
/// # Errors
///
/// As [`decode_row`], plus a malformed or mis-digested `valueDoc`.
pub fn decode_pointer(item: &Item, asserted: WorkspaceId) -> Result<RegistryPointer, CodecError> {
    let collection = decode_row(item, asserted)?;
    let bound = Row::bind(item, REGISTRY_POINTER)?;
    let value_doc = ValueDocument::new(CanonicalJson::parse(bound.string(VALUE_DOC)?).map_err(
        |error| CodecError::Malformed {
            item_type: REGISTRY_POINTER,
            attribute: VALUE_DOC,
            reason: error.to_string(),
        },
    )?);
    if value_doc.digest() != collection.sha256 || value_doc.size_bytes() != collection.size_bytes {
        return Err(CodecError::Malformed {
            item_type: REGISTRY_POINTER,
            attribute: VALUE_DOC,
            reason: "the stored digest and size are not this value document's own".to_owned(),
        });
    }
    Ok(RegistryPointer {
        row: collection,
        value_doc,
    })
}

/// Decodes one `(workspace, kind)` entry count.
///
/// # Errors
///
/// [`CodecError`] for a missing or mistyped count, or a row from another tenant.
pub fn decode_count(item: &Item, asserted: WorkspaceId) -> Result<u64, CodecError> {
    let row = Row::bind(item, REGISTRY_COUNT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    row.u64(COUNT)
}

/// Encodes one staged upload into the rows that carry it.
///
/// The row carries **no TTL**: a timer that removed it would orphan the live
/// multipart upload it names, and the 24-hour sweeper has to abort the upload
/// before the row goes.
///
/// `providerUploadId` and `objectKey` are unconditional. They are what let
/// `upload_abort` issue the exact `AbortMultipartUpload` and what let the expiry
/// sweep run at all; before they were persisted, neither was possible from
/// durable state (E D-1).
#[must_use]
pub fn encode_upload(upload: &Upload) -> UploadRows {
    let key = keys::upload(upload.id);
    let part_count = upload.parts.parts.len();
    let spilled = part_count > PARTS_PER_BLOCK;
    let block_count = if spilled {
        part_count.div_ceil(PARTS_PER_BLOCK)
    } else {
        0
    };

    let mut head = ItemBuilder::new(REGISTRY_UPLOAD)
        .set(PK, s(key.pk.clone()))
        .set(SK, s(key.sk))
        .set("uploadId", s(upload.id.to_string()))
        .set("workspaceId", s(upload.workspace.to_string()))
        .set("targetName", s(upload.target_name.as_str().to_owned()))
        .set("state", s(upload_state_str(upload.state)))
        .set("providerUploadId", s(upload.provider_upload_id.clone()))
        .set("objectKey", s(upload.object_key.clone()))
        .set("declaredSizeBytes", n(upload.declared_size))
        .set("declaredSha256", s(upload.declared_sha256.to_wire()))
        .set_opt(
            "contentType",
            upload.content_type.as_ref().map(|value| s(value.clone())),
        )
        .set("declaredPartCount", n(part_count as u64))
        .set("partBlockCount", n(block_count as u64))
        .set_opt(
            "consumedByKind",
            upload
                .consumed_by
                .as_ref()
                .map(|selector| s(selector.kind.as_str())),
        )
        .set_opt(
            "consumedByName",
            upload
                .consumed_by
                .as_ref()
                .map(|selector| s(selector.name.as_str().to_owned())),
        )
        .set("createdAt", stamp(upload.created_at))
        .set("expiresAt", stamp(upload.expires_at));

    if let Some(evidence) = &upload.completion {
        head = head
            .set("objectEtag", s(evidence.etag.clone()))
            .set_opt(
                "objectChecksumSha256",
                evidence.checksum_sha256.clone().map(s),
            )
            .set_opt(
                "objectChecksumCrc64Nvme",
                evidence.checksum_crc64_nvme.clone().map(s),
            )
            .set_opt("objectPartCount", evidence.part_count.map(n));
    }

    if spilled {
        let mut part_blocks = Vec::with_capacity(block_count);
        for (block, parts) in upload.parts.parts.chunks(PARTS_PER_BLOCK).enumerate() {
            let manifest = upload
                .completion_manifest
                .chunks(PARTS_PER_BLOCK)
                .nth(block)
                .unwrap_or(&[]);
            part_blocks.push(
                ItemBuilder::new(REGISTRY_UPLOAD_PARTS)
                    .set(PK, s(key.pk.clone()))
                    .set(SK, s(keys::upload_parts_sort(block)))
                    .set("uploadId", s(upload.id.to_string()))
                    .set("workspaceId", s(upload.workspace.to_string()))
                    .set("block", n(block as u64))
                    .set(
                        "parts",
                        aex_session_dynamodb::attr::string_list(encode_parts(parts)),
                    )
                    .set(
                        "completionManifest",
                        aex_session_dynamodb::attr::string_list(encode_manifest(manifest)),
                    )
                    .build(),
            );
        }
        return UploadRows {
            head: head.build(),
            part_blocks,
        };
    }

    UploadRows {
        head: head
            .set(
                "parts",
                aex_session_dynamodb::attr::string_list(encode_parts(&upload.parts.parts)),
            )
            .set(
                "completionManifest",
                aex_session_dynamodb::attr::string_list(encode_manifest(
                    &upload.completion_manifest,
                )),
            )
            .build(),
        part_blocks: Vec::new(),
    }
}

fn encode_parts(parts: &[PlannedPart]) -> Vec<String> {
    parts
        .iter()
        .map(|part| match part.sha256 {
            None => format!("{}:{}:", part.number, part.bytes),
            Some(digest) => format!(
                "{}:{}:{}",
                part.number,
                part.bytes,
                hex::encode(digest.as_bytes())
            ),
        })
        .collect()
}

fn encode_manifest(manifest: &[SubmittedPart]) -> Vec<String> {
    manifest
        .iter()
        .map(|part| format!("{}:{}", part.number, part.etag))
        .collect()
}

/// Decodes one staged upload whose part arrays fit in the head row.
///
/// # Errors
///
/// [`CodecError`] as for every decode here, and [`CodecError::Malformed`] naming
/// `partBlockCount` when the upload spilled — a spilled upload has to be read
/// with [`decode_upload_blocks`], because decoding one silently without its
/// blocks would produce a short part plan that looks complete.
pub fn decode_upload(item: &Item, asserted: WorkspaceId) -> Result<Upload, CodecError> {
    decode_upload_blocks(item, &[], asserted)
}

/// Decodes one staged upload from its head row and its part blocks.
///
/// # Errors
///
/// [`CodecError`] as for every decode here, and [`CodecError::Malformed`] when
/// the blocks do not add up to the head row's `declaredPartCount`.
pub fn decode_upload_blocks(
    item: &Item,
    blocks: &[Item],
    asserted: WorkspaceId,
) -> Result<Upload, CodecError> {
    let row = Row::bind(item, REGISTRY_UPLOAD)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let state = upload_state_of(row.enumerated("state", keys::UPLOAD_STATES)?).ok_or(
        CodecError::Malformed {
            item_type: REGISTRY_UPLOAD,
            attribute: "state",
            reason: "outside the upload state vocabulary".to_owned(),
        },
    )?;
    let declared_part_count = usize::try_from(row.u64("declaredPartCount")?).unwrap_or(usize::MAX);
    let block_count = usize::try_from(row.u64("partBlockCount")?).unwrap_or(usize::MAX);

    let (parts, manifest) = decode_part_arrays(item, blocks, block_count, asserted)?;

    if parts.len() != declared_part_count {
        return Err(CodecError::Malformed {
            item_type: REGISTRY_UPLOAD,
            attribute: "declaredPartCount",
            reason: format!(
                "the row declares {declared_part_count} part(s) and {} were decoded",
                parts.len()
            ),
        });
    }

    let completion = decode_completion(item, &row)?;

    let consumed_by = match (
        row.opt_string("consumedByKind")?,
        row.opt_string("consumedByName")?,
    ) {
        (Some(kind), Some(name)) => Some(RegistrySelector {
            workspace: asserted,
            kind: RegistryKind::parse(kind).ok_or(CodecError::Malformed {
                item_type: REGISTRY_UPLOAD,
                attribute: "consumedByKind",
                reason: "outside the registry kind vocabulary".to_owned(),
            })?,
            name: aex_wire::ids::ResourceName::parse(name).map_err(|error| {
                CodecError::Malformed {
                    item_type: REGISTRY_UPLOAD,
                    attribute: "consumedByName",
                    reason: error.to_string(),
                }
            })?,
        }),
        _ => None,
    };
    Ok(Upload {
        id: parse_id::<UploadId>(&row, "uploadId")?,
        target_name: aex_wire::ids::ResourceName::parse(row.string("targetName")?).map_err(
            |error| CodecError::Malformed {
                item_type: REGISTRY_UPLOAD,
                attribute: "targetName",
                reason: error.to_string(),
            },
        )?,
        workspace: asserted,
        state,
        provider_upload_id: row.string("providerUploadId")?.to_owned(),
        object_key: row.string("objectKey")?.to_owned(),
        declared_size: row.u64("declaredSizeBytes")?,
        declared_sha256: content_hash(&row, "declaredSha256")?,
        content_type: row.opt_string("contentType")?.map(str::to_owned),
        parts: PartPlan { parts },
        completion_manifest: manifest,
        completion,
        consumed_by,
        created_at: row.timestamp("createdAt")?,
        expires_at: row.timestamp("expiresAt")?,
    })
}

/// Reads an upload's part arrays from the head row or from its spilled blocks.
///
/// A block whose number is out of order, or a block set that does not match the
/// head row's `partBlockCount`, is refused: reading a short plan as if it were
/// complete is exactly how a completion would silently drop parts.
fn decode_part_arrays(
    item: &Item,
    blocks: &[Item],
    block_count: usize,
    asserted: WorkspaceId,
) -> Result<(Vec<PlannedPart>, Vec<SubmittedPart>), CodecError> {
    if block_count == 0 {
        return Ok((decode_parts(item)?, decode_manifest(item)?));
    }
    if blocks.len() != block_count {
        return Err(CodecError::Malformed {
            item_type: REGISTRY_UPLOAD,
            attribute: "partBlockCount",
            reason: format!(
                "the row names {block_count} part block(s) and {} were supplied",
                blocks.len()
            ),
        });
    }
    let mut parts = Vec::new();
    let mut manifest = Vec::new();
    for (expected, block) in blocks.iter().enumerate() {
        let block_row = Row::bind(block, REGISTRY_UPLOAD_PARTS)?;
        block_row.owned_by("workspaceId", &asserted.to_string())?;
        if usize::try_from(block_row.u64("block")?).unwrap_or(usize::MAX) != expected {
            return Err(CodecError::Malformed {
                item_type: REGISTRY_UPLOAD_PARTS,
                attribute: "block",
                reason: "part blocks must be supplied in block order".to_owned(),
            });
        }
        parts.extend(decode_parts(block)?);
        manifest.extend(decode_manifest(block)?);
    }
    Ok((parts, manifest))
}

/// Reads the completion evidence, when the object it proves exists.
fn decode_completion(item: &Item, row: &Row<'_>) -> Result<Option<CompletionEvidence>, CodecError> {
    let Some(etag) = row.opt_string("objectEtag")? else {
        return Ok(None);
    };
    Ok(Some(CompletionEvidence {
        etag: etag.to_owned(),
        checksum_sha256: row.opt_string("objectChecksumSha256")?.map(str::to_owned),
        checksum_crc64_nvme: row
            .opt_string("objectChecksumCrc64Nvme")?
            .map(str::to_owned),
        part_count: match item.get("objectPartCount") {
            None => None,
            Some(_) => Some(row.u64("objectPartCount")?),
        },
    }))
}

fn string_list<'a>(
    item: &'a Item,
    item_type: &'static str,
    attribute: &'static str,
) -> Result<Vec<&'a str>, CodecError> {
    let encoded = item
        .get(attribute)
        .and_then(|value| value.as_l().ok())
        .ok_or(CodecError::Missing {
            item_type,
            attribute,
        })?;
    encoded
        .iter()
        .map(|entry| {
            entry
                .as_s()
                .map(String::as_str)
                .map_err(|_| CodecError::WrongType {
                    item_type,
                    attribute,
                    expected: "S",
                    found: "another type",
                })
        })
        .collect()
}

fn decode_parts(item: &Item) -> Result<Vec<PlannedPart>, CodecError> {
    let item_type = if item
        .get("itemType")
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        == Some(REGISTRY_UPLOAD_PARTS)
    {
        REGISTRY_UPLOAD_PARTS
    } else {
        REGISTRY_UPLOAD
    };
    let malformed = |reason: &str| CodecError::Malformed {
        item_type,
        attribute: "parts",
        reason: reason.to_owned(),
    };
    let mut parts = Vec::new();
    for text in string_list(item, item_type, "parts")? {
        let mut fields = text.splitn(3, ':');
        let number = fields.next().ok_or_else(|| {
            malformed("a part is `number:bytes:sha256`, with the digest possibly empty")
        })?;
        let bytes = fields.next().ok_or_else(|| {
            malformed("a part is `number:bytes:sha256`, with the digest possibly empty")
        })?;
        let digest = fields.next().ok_or_else(|| {
            malformed("a part is `number:bytes:sha256`, with the digest possibly empty")
        })?;
        parts.push(PlannedPart {
            number: number
                .parse::<u32>()
                .map_err(|_| malformed("a part number is a small integer"))?,
            bytes: bytes
                .parse::<u64>()
                .map_err(|_| malformed("a part size is an integer"))?,
            sha256: if digest.is_empty() {
                None
            } else {
                Some(
                    ContentHash::parse(&format!("sha256:{digest}"))
                        .map_err(|_| malformed("a part digest is 64 lowercase hex characters"))?,
                )
            },
        });
    }
    Ok(parts)
}

fn decode_manifest(item: &Item) -> Result<Vec<SubmittedPart>, CodecError> {
    let item_type = if item
        .get("itemType")
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
        == Some(REGISTRY_UPLOAD_PARTS)
    {
        REGISTRY_UPLOAD_PARTS
    } else {
        REGISTRY_UPLOAD
    };
    let mut manifest = Vec::new();
    for text in string_list(item, item_type, "completionManifest")? {
        let (number, etag) = text.split_once(':').ok_or(CodecError::Malformed {
            item_type,
            attribute: "completionManifest",
            reason: "a manifest entry is `number:etag`".to_owned(),
        })?;
        manifest.push(SubmittedPart {
            number: number.parse::<u32>().map_err(|_| CodecError::Malformed {
                item_type,
                attribute: "completionManifest",
                reason: "a part number is a small integer".to_owned(),
            })?,
            etag: etag.to_owned(),
        });
    }
    Ok(manifest)
}

/// The TTL a receipt carries: 24 hours after it committed (OD-16).
#[must_use]
pub fn receipt_ttl(expires_at: Timestamp) -> aws_sdk_dynamodb::types::AttributeValue {
    n_i64(expires_at.unix_millis().div_euclid(1_000))
}

/// Encodes one idempotency receipt.
///
/// This is a thin re-export of `aex_session_dynamodb::replay::encode_receipt_row`
/// rather than a second implementation. `regional-registry` declares the
/// `idempotency_receipt` item type and a 24-hour TTL scoped to it, but this crate
/// had **no** receipt codec at all, so `upload_create` and `upload_complete` had
/// no way to be idempotent. A second spelling of a receipt would be a second,
/// subtly different idempotency implementation — which is exactly what the shared
/// combinator exists to prevent (E D-19).
///
/// # Errors
///
/// [`KeyError`] when the rendered scope or the key digest could not enter a key.
pub fn encode_receipt(workspace: WorkspaceId, receipt: &Receipt) -> Result<Item, KeyError> {
    encode_receipt_row(workspace, receipt)
}

/// Decodes one idempotency receipt.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or malformed attribute.
pub fn decode_receipt(item: &Item) -> Result<Receipt, CodecError> {
    decode_receipt_row(item)
}

/// Whether a receipt is still readable at `now`.
///
/// The explicit `expiresAt` is the fence, never the TTL attribute: AWS reclaims a
/// TTL'd row within 48 hours, not at the instant.
#[must_use]
pub fn receipt_is_live(receipt: &Receipt, now: Timestamp) -> bool {
    aex_session_dynamodb::replay::receipt_is_live(receipt, now)
}

fn content_hash(row: &Row<'_>, attribute: &'static str) -> Result<ContentHash, CodecError> {
    let text = row.string(attribute)?;
    ContentHash::parse(text).map_err(|error| CodecError::Malformed {
        item_type: REGISTRY_POINTER,
        attribute,
        reason: error.to_string(),
    })
}

fn parse_id<T: aex_wire::ids::PrefixedId>(
    row: &Row<'_>,
    attribute: &'static str,
) -> Result<T, CodecError> {
    row.id::<T>(attribute)
}

#[cfg(test)]
mod tests {
    use aex_content_domain::identity::{RegistryKind, Revision};
    use aex_session_dynamodb::attr::CodecError;
    use aex_wire::ids::{ContentHash, PrefixedId, ResourceName, UploadId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;
    use aex_workspace_domain::registry::{RegistryPointer, RegistryRow, ValueDocument, etag_of};
    use aex_workspace_domain::upload::{
        CompletionEvidence, PartPlan, PlannedPart, SubmittedPart, Upload, UploadState,
    };

    use super::{
        PARTS_PER_BLOCK, VALUE_DOC, decode_pointer, decode_row, decode_upload,
        decode_upload_blocks, encode_pointer, encode_upload,
    };

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [byte; 10]))
    }

    fn now() -> Timestamp {
        Timestamp::parse("2026-08-01T12:34:56.789Z").expect("the pinned spelling")
    }

    fn value_doc() -> ValueDocument {
        ValueDocument::new(
            aex_wire::CanonicalJson::parse(
                r#"{"description":"search the web","entry":"main.js","bundleFormat":"tar.gz",
                    "inputSchema":{"type":"object"},
                    "bundle":{"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "sizeBytes":"2048"}}"#,
            )
            .expect("valid JSON"),
        )
    }

    fn pointer() -> RegistryPointer {
        let document = value_doc();
        let digest = document.digest();
        RegistryPointer {
            row: RegistryRow {
                workspace: workspace(1),
                kind: RegistryKind::Tool,
                name: ResourceName::parse("search").expect("a name"),
                revision: Revision::FIRST,
                etag: etag_of(RegistryKind::Tool, Revision::FIRST, &digest),
                sha256: digest,
                size_bytes: document.size_bytes(),
                state: aex_workspace_domain::registry::RegistryState::Ready,
                failure_code: None,
                created_at: now(),
                updated_at: now(),
            },
            value_doc: document,
        }
    }

    fn upload() -> Upload {
        Upload {
            id: UploadId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10])),
            target_name: ResourceName::parse("artifact").expect("a name"),
            workspace: workspace(1),
            state: UploadState::PartsGranted,
            provider_upload_id: "provider-mpu-1".to_owned(),
            object_key: "wks/ab/cd/abcd".to_owned(),
            declared_size: 6 * 1024 * 1024,
            declared_sha256: ContentHash::from_bytes([2; 32]),
            content_type: Some("application/zip".to_owned()),
            parts: PartPlan {
                parts: vec![
                    PlannedPart {
                        number: 1,
                        bytes: 5 * 1024 * 1024,
                        sha256: Some(ContentHash::from_bytes([3; 32])),
                    },
                    PlannedPart {
                        number: 2,
                        bytes: 1024 * 1024,
                        sha256: None,
                    },
                ],
            },
            completion_manifest: vec![SubmittedPart {
                number: 1,
                etag: "\"one\"".to_owned(),
            }],
            completion: Some(CompletionEvidence {
                etag: "\"assembled\"".to_owned(),
                checksum_sha256: Some("composite=".to_owned()),
                checksum_crc64_nvme: None,
                part_count: Some(2),
            }),
            consumed_by: None,
            created_at: now(),
            expires_at: now(),
        }
    }

    /// One upload wide enough to spill its part arrays into sibling blocks.
    fn spilled_upload() -> Upload {
        let mut wide = upload();
        wide.parts = PartPlan {
            parts: (1..=u32::try_from(PARTS_PER_BLOCK + 1).expect("bounded"))
                .map(|number| PlannedPart {
                    number,
                    bytes: 5 * 1024 * 1024,
                    sha256: Some(ContentHash::from_bytes(
                        [u8::try_from(number % 251).expect("bounded"); 32],
                    )),
                })
                .collect(),
        };
        wide.completion_manifest = (1..=u32::try_from(PARTS_PER_BLOCK + 1).expect("bounded"))
            .map(|number| SubmittedPart {
                number,
                etag: format!("\"{number}\""),
            })
            .collect();
        wide
    }

    #[test]
    fn a_pointer_round_trips_exactly() {
        let original = pointer();
        let encoded = encode_pointer(&original).expect("encodes");
        assert_eq!(
            decode_pointer(&encoded, original.row.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn a_stored_etag_that_disagrees_with_its_own_row_is_corruption_not_a_value() {
        let mut encoded = encode_pointer(&pointer()).expect("encodes");
        encoded.insert(
            "etag".to_owned(),
            aex_session_dynamodb::attr::s("0".repeat(32)),
        );
        let error = decode_pointer(&encoded, workspace(1)).expect_err("a forged tag");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_revision_bump_changes_the_tag_so_a_client_can_never_reuse_a_stale_one() {
        let first = pointer();
        let mut second = first.clone();
        second.row.revision = first.row.revision.next();
        second.row.etag = etag_of(second.row.kind, second.row.revision, &second.row.sha256);
        assert_ne!(first.row.etag, second.row.etag);
    }

    #[test]
    fn a_pointer_row_stores_no_upload_and_no_value_reference() {
        let encoded = encode_pointer(&pointer()).expect("encodes");
        assert!(
            !encoded.contains_key("valueKind") && !encoded.contains_key("valueId"),
            "a durable pointer names a digest and nothing else (D-15)"
        );
        assert!(encoded.contains_key(VALUE_DOC));
    }

    #[test]
    fn a_value_document_the_stored_digest_does_not_describe_is_corruption() {
        let mut encoded = encode_pointer(&pointer()).expect("encodes");
        encoded.insert(
            VALUE_DOC.to_owned(),
            aex_session_dynamodb::attr::s(r#"{"description":"edited behind the digest"}"#),
        );
        let error = decode_pointer(&encoded, workspace(1)).expect_err("a forged document");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_collection_row_decodes_without_the_value_document_at_all() {
        let mut encoded = encode_pointer(&pointer()).expect("encodes");
        encoded.remove(VALUE_DOC);
        let row = decode_row(&encoded, workspace(1)).expect("decodes a projected row");
        assert_eq!(row, pointer().row);
        assert!(decode_pointer(&encoded, workspace(1)).is_err());
    }

    #[test]
    fn an_upload_round_trips_with_its_whole_part_plan() {
        let original = upload();
        let rows = encode_upload(&original);
        assert!(rows.part_blocks.is_empty(), "a two-part upload is one item");
        assert_eq!(
            decode_upload(&rows.head, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn an_upload_names_its_provider_handle_and_its_object_key() {
        let rows = encode_upload(&upload());
        assert!(rows.head.contains_key("providerUploadId"));
        assert!(rows.head.contains_key("objectKey"));
    }

    #[test]
    fn a_row_missing_its_provider_handle_can_never_be_decoded() {
        // The mirror of the TTL test below: a row without a handle cannot be
        // aborted and cannot be swept, so it must not be readable as an upload
        // at all rather than surfacing as a plausible one.
        let mut head = encode_upload(&upload()).head;
        head.remove("providerUploadId");
        let error = decode_upload(&head, workspace(1)).expect_err("no handle");
        assert!(matches!(error, CodecError::Missing { .. }), "{error}");
    }

    #[test]
    fn a_wide_upload_spills_its_part_arrays_and_still_round_trips() {
        let original = spilled_upload();
        let rows = encode_upload(&original);
        assert_eq!(rows.part_blocks.len(), 2, "1001 parts is two blocks");
        assert!(
            !rows.head.contains_key("parts"),
            "a spilled upload keeps no partial plan on the head row"
        );
        assert_eq!(
            decode_upload_blocks(&rows.head, &rows.part_blocks, original.workspace)
                .expect("decodes"),
            original
        );
    }

    #[test]
    fn a_spilled_upload_is_never_decoded_from_its_head_alone() {
        // Decoding the head by itself would produce an empty part plan that looks
        // like a complete one, which is exactly how a completion would silently
        // drop parts.
        let rows = encode_upload(&spilled_upload());
        let error = decode_upload(&rows.head, workspace(1)).expect_err("blocks missing");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn an_upload_row_never_carries_a_ttl_attribute() {
        let encoded = encode_upload(&upload()).head;
        assert!(
            !encoded.contains_key("expiresAtEpochSeconds"),
            "a TTL here would orphan the live multipart upload the row names"
        );
        assert!(encoded.contains_key("expiresAt"));
    }

    #[test]
    fn a_row_from_another_tenant_is_refused_after_read() {
        let encoded = encode_pointer(&pointer()).expect("encodes");
        let error = decode_pointer(&encoded, workspace(9)).expect_err("another tenant");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }
}
