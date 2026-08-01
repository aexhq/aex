//! `regional-work` row codecs.
//!
//! The payload check is the load-bearing one. This table's stream is
//! `NEW_IMAGE`, which duplicates every written row into stream storage and into
//! the pipe role's blast radius. That is safe only because a work payload is
//! defined to hold typed non-content metadata, and this codec is what turns that
//! definition into an enforced property: an over-large payload, or one carrying
//! a key outside the declared per-kind schema, is rejected at encode time.

use std::collections::BTreeMap;

use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, Row, boolean, n, n_i64, s, stamp};
use aex_session_dynamodb::component::KeyError;
use aex_wire::ids::{AgentId, OrganizationId, SessionId, WorkspaceId};
use aex_wire::types::Timestamp;
use aws_sdk_dynamodb::types::AttributeValue;

use crate::keys;

/// The `itemType` of a work record.
pub const WORK: &str = "work";
/// The `itemType` of a dedupe claim.
pub const WORK_DEDUPE: &str = "work_dedupe";
/// The `itemType` of a reconciliation cursor.
pub const WORK_CURSOR: &str = "work_cursor";

/// The attribute names a work payload may use, per `kind`.
///
/// A payload key outside its kind's list is rejected. The lists are exhaustive
/// on purpose: adding a field is a deliberate edit here, which is what stops the
/// payload from slowly becoming a place to put things.
#[must_use]
pub fn payload_schema(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind {
        "agent.wake" => &["sessionId", "agentId", "fromSeq", "cancelEpoch"],
        "operation.step" => &["operationId", "sessionId", "version"],
        "content.gc_mark" | "content.gc_sweep" => &["workspaceId", "epoch", "bucket"],
        "content.staged_orphan" => &["workspaceId", "digest"],
        "registry.upload_expiry" => &["uploadId", "workspaceId"],
        "runtime.evaluate" => &["sessionId", "generationId", "fence"],
        "usage.storage.delta" => &["workspaceId", "digest", "deltaBytes", "residenceId", "at"],
        "usage.compute.closure" => &["workspaceId", "runId", "closureId"],
        "usage.transfer.authorized" => &["workspaceId", "measurementId", "authorizedBytes"],
        "secret.lineage_sweep" => &["workspaceId", "name", "throughRevision"],
        _ => return None,
    })
}

/// A typed, bounded work payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Payload(BTreeMap<String, String>);

impl Payload {
    /// An empty payload.
    #[must_use]
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    /// Sets one member.
    #[must_use]
    pub fn set(mut self, name: &str, value: impl Into<String>) -> Self {
        self.0.insert(name.to_owned(), value.into());
        self
    }

    /// The members.
    #[must_use]
    pub const fn members(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Checks the payload against the declared schema for `kind`.
    ///
    /// # Errors
    ///
    /// [`PayloadError::UnknownKind`] for a kind outside the closed vocabulary,
    /// [`PayloadError::UndeclaredMember`] for a key the kind does not declare,
    /// and [`PayloadError::TooLarge`] above [`keys::MAX_PAYLOAD_BYTES`].
    pub fn check(&self, kind: &str) -> Result<(), PayloadError> {
        let declared = payload_schema(kind).ok_or(PayloadError::UnknownKind {
            kind: kind.to_owned(),
        })?;
        for name in self.0.keys() {
            if !declared.contains(&name.as_str()) {
                return Err(PayloadError::UndeclaredMember {
                    kind: kind.to_owned(),
                    member: name.clone(),
                });
            }
        }
        let measured: usize = self
            .0
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum();
        if measured > keys::MAX_PAYLOAD_BYTES {
            return Err(PayloadError::TooLarge { measured });
        }
        Ok(())
    }

    fn encode(&self) -> AttributeValue {
        AttributeValue::M(
            self.0
                .iter()
                .map(|(name, value)| (name.clone(), s(value.clone())))
                .collect(),
        )
    }

    fn decode(value: &AttributeValue) -> Result<Self, CodecError> {
        let members = value.as_m().map_err(|_| CodecError::WrongType {
            item_type: WORK,
            attribute: "payload",
            expected: "M",
            found: "another type",
        })?;
        let mut decoded = BTreeMap::new();
        for (name, member) in members {
            let text = member.as_s().map_err(|_| CodecError::Malformed {
                item_type: WORK,
                attribute: "payload",
                reason: format!("member `{name}` is not a string"),
            })?;
            decoded.insert(name.clone(), text.clone());
        }
        Ok(Self(decoded))
    }
}

/// Why a payload was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PayloadError {
    /// The kind is outside the closed vocabulary.
    #[error("`{kind}` is not a work kind")]
    UnknownKind {
        /// The rejected kind.
        kind: String,
    },
    /// The payload carries a member the kind never declares.
    #[error("work kind `{kind}` does not declare payload member `{member}`")]
    UndeclaredMember {
        /// The kind.
        kind: String,
        /// The offending member.
        member: String,
    },
    /// The payload exceeds the declared ceiling.
    #[error(
        "a work payload measures {measured} bytes; the ceiling is {}",
        keys::MAX_PAYLOAD_BYTES
    )]
    TooLarge {
        /// The measured size.
        measured: usize,
    },
}

/// Evidence that the wake was published, kept so a redelivery is explainable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryEvidence {
    /// How many times the record has been projected onto a queue.
    pub publish_count: u64,
    /// When it was last projected.
    pub last_published_at: Option<Timestamp>,
    /// The last receipt the worker acked.
    pub last_receipt_id: Option<String>,
}

/// One durable unit of runnable work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkRecord {
    /// Its identity.
    pub work_id: String,
    /// Its workspace.
    pub workspace: WorkspaceId,
    /// Its organization.
    pub organization: OrganizationId,
    /// The session it belongs to, when it belongs to one.
    pub session: Option<SessionId>,
    /// The agent it belongs to, when it belongs to one.
    pub agent: Option<AgentId>,
    /// Its kind, from the closed vocabulary.
    pub kind: String,
    /// Its priority band, zero highest.
    pub priority: u8,
    /// When it becomes due.
    pub due_at: Timestamp,
    /// Its state.
    pub state: String,
    /// How many attempts have been started.
    pub attempt: u64,
    /// How many are permitted.
    pub max_attempts: u64,
    /// The monotonic claim fence.
    pub fence: u64,
    /// The current claim owner.
    pub claim_owner: Option<String>,
    /// When the current lease expires.
    pub lease_expires_at: Option<Timestamp>,
    /// The hashed dedupe key.
    pub dedupe_key: String,
    /// The typed payload.
    pub payload: Payload,
    /// Delivery evidence.
    pub delivery: DeliveryEvidence,
    /// When it was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// Encodes one work record, including its sparse due-index attributes.
///
/// A `done` or `poisoned` record carries neither index attribute, so the index
/// holds only outstanding work.
///
/// # Errors
///
/// [`EncodeError`] when the payload is refused or a key component is unusable.
pub fn encode_work(record: &WorkRecord) -> Result<Item, EncodeError> {
    record.payload.check(&record.kind)?;
    if !keys::KINDS.contains(&record.kind.as_str()) {
        return Err(EncodeError::Payload(PayloadError::UnknownKind {
            kind: record.kind.clone(),
        }));
    }
    let key = keys::work(&record.work_id)?;
    let effective = keys::effective_due_at(record.due_at, record.priority)?;
    let outstanding = record.state == "pending" || record.state == "claimed";
    let builder = ItemBuilder::new(WORK)
        .set(aex_session_dynamodb::attr::PK, s(key.pk))
        .set(aex_session_dynamodb::attr::SK, s(key.sk))
        .set("workId", s(record.work_id.clone()))
        .set("workspaceId", s(record.workspace.to_string()))
        .set("organizationId", s(record.organization.to_string()))
        .set_opt(
            "sessionId",
            record.session.map(|session| s(session.to_string())),
        )
        .set_opt("agentId", record.agent.map(|agent| s(agent.to_string())))
        .set("kind", s(record.kind.clone()))
        .set("priority", n(u64::from(record.priority)))
        .set("dueAt", stamp(record.due_at))
        .set("effectiveDueAt", stamp(effective))
        .set("state", s(record.state.clone()))
        .set("attempt", n(record.attempt))
        .set("maxAttempts", n(record.max_attempts))
        .set("fence", n(record.fence))
        .set_opt(
            "claimOwner",
            record.claim_owner.as_ref().map(|owner| s(owner.clone())),
        )
        .set_opt("leaseExpiresAt", record.lease_expires_at.map(stamp))
        .set("dedupeKey", s(record.dedupe_key.clone()))
        .set("payload", record.payload.encode())
        .set("publishCount", n(record.delivery.publish_count))
        .set_opt(
            "lastPublishedAt",
            record.delivery.last_published_at.map(stamp),
        )
        .set_opt(
            "lastReceiptId",
            record
                .delivery
                .last_receipt_id
                .as_ref()
                .map(|receipt| s(receipt.clone())),
        )
        .set("createdAt", stamp(record.created_at))
        .set("updatedAt", stamp(record.updated_at));
    let builder = if outstanding {
        builder
            .set(keys::DUE_PK, s(keys::due_partition(&record.work_id)))
            .set(keys::DUE_SK, s(keys::due_sort(effective, &record.work_id)?))
    } else {
        builder
    };
    Ok(builder.build())
}

/// Why a row could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// The payload was refused.
    #[error(transparent)]
    Payload(#[from] PayloadError),
    /// A key component was unusable.
    #[error(transparent)]
    Key(#[from] KeyError),
}

/// Decodes one work record and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, a
/// payload carrying an undeclared member, or a row from another tenant.
pub fn decode_work(item: &Item, asserted: WorkspaceId) -> Result<WorkRecord, CodecError> {
    let row = Row::bind(item, WORK)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let kind = row.enumerated("kind", keys::KINDS)?;
    let payload = Payload::decode(item.get("payload").ok_or(CodecError::Missing {
        item_type: WORK,
        attribute: "payload",
    })?)?;
    // The payload schema is re-checked on the way out too: a row written by an
    // older revision, or by hand, must not become readable just because it is
    // already stored.
    payload.check(kind).map_err(|error| CodecError::Malformed {
        item_type: WORK,
        attribute: "payload",
        reason: error.to_string(),
    })?;
    let priority = u8::try_from(row.u64("priority")?).map_err(|_| CodecError::Malformed {
        item_type: WORK,
        attribute: "priority",
        reason: "a priority band is a small integer".to_owned(),
    })?;
    Ok(WorkRecord {
        work_id: row.string("workId")?.to_owned(),
        workspace: asserted,
        organization: row.id::<OrganizationId>("organizationId")?,
        session: row.opt_id::<SessionId>("sessionId")?,
        agent: row.opt_id::<AgentId>("agentId")?,
        kind: kind.to_owned(),
        priority,
        due_at: row.timestamp("dueAt")?,
        state: row.enumerated("state", keys::STATES)?.to_owned(),
        attempt: row.u64("attempt")?,
        max_attempts: row.u64("maxAttempts")?,
        fence: row.u64("fence")?,
        claim_owner: row.opt_string("claimOwner")?.map(str::to_owned),
        lease_expires_at: row.opt_timestamp("leaseExpiresAt")?,
        dedupe_key: row.string("dedupeKey")?.to_owned(),
        payload,
        delivery: DeliveryEvidence {
            publish_count: row.u64("publishCount")?,
            last_published_at: row.opt_timestamp("lastPublishedAt")?,
            last_receipt_id: row.opt_string("lastReceiptId")?.map(str::to_owned),
        },
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// Encodes the dedupe claim written in the same transaction as the record.
///
/// # Errors
///
/// [`KeyError`] when the digest could not enter a key.
pub fn encode_dedupe(
    dedupe_key_sha256_hex: &str,
    work_id: &str,
    created_at: Timestamp,
) -> Result<Item, KeyError> {
    let key = keys::dedupe(dedupe_key_sha256_hex)?;
    Ok(ItemBuilder::new(WORK_DEDUPE)
        .set(aex_session_dynamodb::attr::PK, s(key.pk))
        .set(aex_session_dynamodb::attr::SK, s(key.sk))
        .set("workId", s(work_id.to_owned()))
        .set("dedupeKey", s(dedupe_key_sha256_hex.to_owned()))
        .set("createdAt", stamp(created_at))
        .build())
}

/// One shard's reconciliation cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciliationCursor {
    /// Which shard.
    pub shard: u16,
    /// How far the reconciler has swept.
    pub scanned_through_effective_due_at: Timestamp,
    /// The last work identity it saw.
    pub last_work_id: Option<String>,
    /// Its optimistic revision.
    pub revision: u64,
    /// When it last advanced.
    pub updated_at: Timestamp,
}

/// Encodes one reconciliation cursor.
#[must_use]
pub fn encode_cursor(cursor: &ReconciliationCursor) -> Item {
    let key = keys::cursor(cursor.shard);
    ItemBuilder::new(WORK_CURSOR)
        .set(aex_session_dynamodb::attr::PK, s(key.pk))
        .set(aex_session_dynamodb::attr::SK, s(key.sk))
        .set("shard", n(u64::from(cursor.shard)))
        .set(
            "scannedThroughEffectiveDueAt",
            stamp(cursor.scanned_through_effective_due_at),
        )
        .set_opt(
            "lastWorkId",
            cursor.last_work_id.as_ref().map(|id| s(id.clone())),
        )
        .set("revision", n(cursor.revision))
        .set("updatedAt", stamp(cursor.updated_at))
        .build()
}

/// Decodes one reconciliation cursor.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_cursor(item: &Item) -> Result<ReconciliationCursor, CodecError> {
    let row = Row::bind(item, WORK_CURSOR)?;
    let shard = u16::try_from(row.u64("shard")?).map_err(|_| CodecError::Malformed {
        item_type: WORK_CURSOR,
        attribute: "shard",
        reason: "a shard index is a small integer".to_owned(),
    })?;
    Ok(ReconciliationCursor {
        shard,
        scanned_through_effective_due_at: row.timestamp("scannedThroughEffectiveDueAt")?,
        last_work_id: row.opt_string("lastWorkId")?.map(str::to_owned),
        revision: row.u64("revision")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// The TTL value a retired record carries: 24 hours after it settled.
///
/// A redelivered hint inside the window finds `state = done` and acks; after the
/// window it finds nothing and the domain re-derives a no-op from the session or
/// agent revision. Both arms are correct, so TTL timing is not a fence.
#[must_use]
pub fn retirement_ttl(updated_at: Timestamp) -> AttributeValue {
    n_i64(updated_at.unix_millis().div_euclid(1_000) + 24 * 60 * 60)
}

/// Whether a state keeps the record in the due index.
#[must_use]
pub fn is_outstanding(state: &str) -> AttributeValue {
    boolean(state == "pending" || state == "claimed")
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OrganizationId, PrefixedId, SessionId, Uuid7, WorkspaceId};
    use aex_wire::types::Timestamp;

    use super::{
        DeliveryEvidence, EncodeError, Payload, PayloadError, WorkRecord, decode_work, encode_work,
    };
    use crate::keys;
    use aex_session_dynamodb::attr::CodecError;

    fn stamp(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn record() -> WorkRecord {
        WorkRecord {
            work_id: "wrk_01".to_owned(),
            workspace: workspace(1),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            session: Some(SessionId::from_uuid7(Uuid7::compose(1, [3; 10]))),
            agent: None,
            kind: "agent.wake".to_owned(),
            priority: 0,
            due_at: stamp(1_000_000),
            state: "pending".to_owned(),
            attempt: 0,
            max_attempts: 5,
            fence: 0,
            claim_owner: None,
            lease_expires_at: None,
            dedupe_key: "a".repeat(64),
            payload: Payload::new().set("fromSeq", "10"),
            delivery: DeliveryEvidence::default(),
            created_at: stamp(999_000),
            updated_at: stamp(999_000),
        }
    }

    #[test]
    fn a_work_record_round_trips_exactly() {
        let original = record();
        let encoded = encode_work(&original).expect("encodes");
        assert_eq!(
            decode_work(&encoded, original.workspace).expect("decodes"),
            original
        );
    }

    #[test]
    fn an_outstanding_record_carries_the_due_index_and_a_retired_one_does_not() {
        let pending = encode_work(&record()).expect("encodes");
        assert!(pending.contains_key(keys::DUE_PK));
        assert!(pending.contains_key(keys::DUE_SK));

        let mut done = record();
        done.state = "done".to_owned();
        let retired = encode_work(&done).expect("encodes");
        assert!(
            !retired.contains_key(keys::DUE_PK),
            "the due index must hold only outstanding work"
        );
        assert!(!retired.contains_key(keys::DUE_SK));

        let mut poisoned = record();
        poisoned.state = "poisoned".to_owned();
        assert!(
            !encode_work(&poisoned)
                .expect("encodes")
                .contains_key(keys::DUE_PK)
        );
    }

    #[test]
    fn a_payload_member_the_kind_never_declares_is_refused() {
        let mut smuggled = record();
        smuggled.payload = Payload::new().set("prompt", "the user said something private");
        let error = encode_work(&smuggled).expect_err("an undeclared member");
        assert!(
            matches!(
                error,
                EncodeError::Payload(PayloadError::UndeclaredMember { .. })
            ),
            "{error}"
        );
    }

    #[test]
    fn an_over_large_payload_is_refused_so_the_new_image_stream_stays_metadata_only() {
        let mut bloated = record();
        bloated.payload = Payload::new().set("fromSeq", "x".repeat(keys::MAX_PAYLOAD_BYTES + 1));
        let error = bloated.payload.check("agent.wake").expect_err("too large");
        assert!(matches!(error, PayloadError::TooLarge { .. }), "{error}");
    }

    #[test]
    fn a_kind_outside_the_closed_vocabulary_never_reaches_a_row() {
        let mut invented = record();
        invented.kind = "agent.teleport".to_owned();
        assert!(encode_work(&invented).is_err());
    }

    #[test]
    fn a_stored_payload_that_violates_its_schema_is_rejected_on_the_way_out_too() {
        let mut encoded = encode_work(&record()).expect("encodes");
        encoded.insert(
            "payload".to_owned(),
            aws_sdk_dynamodb::types::AttributeValue::M(std::collections::HashMap::from([(
                "secretValue".to_owned(),
                aex_session_dynamodb::attr::s("hunter2"),
            )])),
        );
        let error = decode_work(&encoded, workspace(1)).expect_err("an undeclared member");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }

    #[test]
    fn a_record_from_another_tenant_is_rejected_after_read() {
        let encoded = encode_work(&record()).expect("encodes");
        let error = decode_work(&encoded, workspace(9)).expect_err("another tenant");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }
}
