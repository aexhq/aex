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
use aex_wire::CanonicalJson;
use aex_wire::ids::{ContentHash, UploadId, WorkspaceId};
use aex_wire::types::{ETag, Timestamp};
use aex_workspace_domain::registry::{RegistryPointer, RegistryRow, ValueDocument, etag_of};
use aex_workspace_domain::upload::{PartPlan, PlannedPart, RegistrySelector, Upload, UploadState};

use crate::keys;

/// The `itemType` of a current pointer.
pub const REGISTRY_POINTER: &str = "registry_pointer";
/// The `itemType` of a staged upload.
pub const REGISTRY_UPLOAD: &str = "registry_upload";
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
pub const ROW_PROJECTION: &str =
    "itemType, workspaceId, kind, #name, revision, etag, contentDigest, sizeBytes, createdAt, \
     updatedAt";

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

/// Encodes one staged upload.
///
/// The row carries **no TTL**: a timer that removed it would orphan the live
/// multipart upload it names, and the 24-hour sweeper has to abort the upload
/// before the row goes.
#[must_use]
pub fn encode_upload(upload: &Upload) -> Item {
    let key = keys::upload(upload.id);
    let parts: Vec<String> = upload
        .parts
        .parts
        .iter()
        .map(|part| format!("{}:{}", part.number, part.bytes))
        .collect();
    ItemBuilder::new(REGISTRY_UPLOAD)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("uploadId", s(upload.id.to_string()))
        .set("workspaceId", s(upload.workspace.to_string()))
        .set("state", s(upload_state_str(upload.state)))
        .set("declaredSizeBytes", n(upload.declared_size))
        .set("declaredSha256", s(upload.declared_sha256.to_wire()))
        .set_opt(
            "contentType",
            upload.content_type.as_ref().map(|value| s(value.clone())),
        )
        .set(
            "parts",
            aex_session_dynamodb::attr::string_list(parts.clone()),
        )
        .set("declaredPartCount", n(parts.len() as u64))
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
        .set("expiresAt", stamp(upload.expires_at))
        .build()
}

/// Decodes one staged upload.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_upload(item: &Item, asserted: WorkspaceId) -> Result<Upload, CodecError> {
    let row = Row::bind(item, REGISTRY_UPLOAD)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let state = upload_state_of(row.enumerated("state", keys::UPLOAD_STATES)?).ok_or(
        CodecError::Malformed {
            item_type: REGISTRY_UPLOAD,
            attribute: "state",
            reason: "outside the upload state vocabulary".to_owned(),
        },
    )?;
    let encoded = item
        .get("parts")
        .and_then(|value| value.as_l().ok())
        .ok_or(CodecError::Missing {
            item_type: REGISTRY_UPLOAD,
            attribute: "parts",
        })?;
    let mut parts = Vec::with_capacity(encoded.len());
    for entry in encoded {
        let text = entry.as_s().map_err(|_| CodecError::WrongType {
            item_type: REGISTRY_UPLOAD,
            attribute: "parts",
            expected: "S",
            found: "another type",
        })?;
        let (number, bytes) = text.split_once(':').ok_or(CodecError::Malformed {
            item_type: REGISTRY_UPLOAD,
            attribute: "parts",
            reason: "a part is `number:bytes`".to_owned(),
        })?;
        parts.push(PlannedPart {
            number: number.parse::<u32>().map_err(|_| CodecError::Malformed {
                item_type: REGISTRY_UPLOAD,
                attribute: "parts",
                reason: "a part number is a small integer".to_owned(),
            })?,
            bytes: bytes.parse::<u64>().map_err(|_| CodecError::Malformed {
                item_type: REGISTRY_UPLOAD,
                attribute: "parts",
                reason: "a part size is an integer".to_owned(),
            })?,
        });
    }
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
        workspace: asserted,
        state,
        declared_size: row.u64("declaredSizeBytes")?,
        declared_sha256: content_hash(&row, "declaredSha256")?,
        content_type: row.opt_string("contentType")?.map(str::to_owned),
        parts: PartPlan { parts },
        consumed_by,
        created_at: row.timestamp("createdAt")?,
        expires_at: row.timestamp("expiresAt")?,
    })
}

/// The TTL a receipt carries: 24 hours after it committed (OD-16).
#[must_use]
pub fn receipt_ttl(expires_at: Timestamp) -> aws_sdk_dynamodb::types::AttributeValue {
    n_i64(expires_at.unix_millis().div_euclid(1_000))
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
    use aex_workspace_domain::registry::{
        RegistryPointer, RegistryRow, ValueDocument, etag_of,
    };
    use aex_workspace_domain::upload::{PartPlan, PlannedPart, Upload, UploadState};

    use super::{VALUE_DOC, decode_pointer, decode_row, decode_upload, encode_pointer, encode_upload};

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
                created_at: now(),
                updated_at: now(),
            },
            value_doc: document,
        }
    }

    fn upload() -> Upload {
        Upload {
            id: UploadId::from_uuid7(Uuid7::compose(1_754_051_696_789, [4; 10])),
            workspace: workspace(1),
            state: UploadState::PartsGranted,
            declared_size: 6 * 1024 * 1024,
            declared_sha256: ContentHash::from_bytes([2; 32]),
            content_type: Some("application/zip".to_owned()),
            parts: PartPlan {
                parts: vec![
                    PlannedPart {
                        number: 1,
                        bytes: 5 * 1024 * 1024,
                    },
                    PlannedPart {
                        number: 2,
                        bytes: 1024 * 1024,
                    },
                ],
            },
            consumed_by: None,
            created_at: now(),
            expires_at: now(),
        }
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
        let encoded = encode_upload(&original);
        assert_eq!(
            decode_upload(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn an_upload_row_never_carries_a_ttl_attribute() {
        let encoded = encode_upload(&upload());
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
