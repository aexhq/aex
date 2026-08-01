//! The authority's own change feed, and this category's receipt queue body.
//!
//! # Why the stream decode lives here
//!
//! The table declares `NEW_IMAGE` and an event-source mapping that filters
//! `itemType = usage_fact` on `INSERT`. Both halves of that contract are this
//! adapter's, so both are enforced here rather than trusted.
//!
//! A record that satisfies the contract but whose image will not decode is
//! **not** a failure of the batch: it is a poisoned record, and it is handed up
//! as [`UndecodableRecord`] so the worker can park the workspace's frontier
//! rather than retry a permanent failure forever.
//!
//! A record that does not satisfy the contract at all — a `MODIFY`, a `REMOVE`,
//! or a frontier row — is a different thing entirely: the event-source filter is
//! misconfigured. Parking a customer's frontier for an infrastructure mistake
//! would be the wrong blame, so the invocation fails and says which record and
//! which filter.

use aex_usage_application::ports::SettlementReceipt;
use aex_usage_application::worker::{ReceiptRecord, StreamRecord, UndecodableRecord};
use aex_usage_domain::frontier::AcceptedSequence;
use aex_usage_domain::identity::FactId;
use aex_usage_domain::keys::ItemType;
use aex_usage_domain::wire_pending::{PricingVersion, RegionId, Timestamp, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::CATEGORY;
use crate::attribute::from_stream_json;
use crate::codec::decode_fact;

/// The only stream event this worker serves.
pub const INSERT: &str = "INSERT";

/// The partition key prefix a workspace partition carries.
const PARTITION_PREFIX: &str = "WS#";
/// The sort key prefix a fact row carries.
const FACT_PREFIX: &str = "FACT#";

/// Why a batch could not be read at all.
///
/// Every variant means the wiring is wrong, never that one customer's record is
/// bad. That distinction is why these are raised rather than quarantined.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StreamError {
    /// The envelope was not a stream batch.
    #[error("the event is not a `Records` batch")]
    NotABatch,
    /// A record was missing a field every stream record carries.
    #[error("stream record {index} carries no `{field}`")]
    Incomplete {
        /// Which record.
        index: usize,
        /// What it was missing.
        field: &'static str,
    },
    /// The event-source filter let a record through that it must exclude.
    ///
    /// Not quarantined: parking a customer's frontier for a misconfigured
    /// filter would blame the wrong thing.
    #[error(
        "stream record {index} is `{event_name}` of `{item_type}`; the event-source \
         mapping must filter `{INSERT}` of `{expected}` only"
    )]
    Unfiltered {
        /// Which record.
        index: usize,
        /// What arrived.
        event_name: String,
        /// What kind of row it was.
        item_type: String,
        /// What the mapping is supposed to deliver.
        expected: &'static str,
    },
}

/// Reads one stream batch into the records the worker folds.
///
/// # Errors
///
/// Returns [`StreamError`] when the envelope or the event-source filter is
/// wrong. A record that is well-formed but undecodable is returned inside the
/// batch as [`UndecodableRecord`], not raised.
pub fn stream_records(event: &serde_json::Value) -> Result<Vec<StreamRecord>, StreamError> {
    let records = event
        .get("Records")
        .and_then(serde_json::Value::as_array)
        .ok_or(StreamError::NotABatch)?;
    let mut decoded = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        decoded.push(one_record(index, record)?);
    }
    Ok(decoded)
}

/// Reads one stream record.
fn one_record(index: usize, record: &serde_json::Value) -> Result<StreamRecord, StreamError> {
    let field = |field: &'static str| StreamError::Incomplete { index, field };

    let event_name = record
        .get("eventName")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| field("eventName"))?;
    let change = record.get("dynamodb").ok_or_else(|| field("dynamodb"))?;
    let identifier = change
        .get("SequenceNumber")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| field("dynamodb.SequenceNumber"))?
        .to_owned();
    let image = change.get("NewImage").ok_or_else(|| field("NewImage"))?;

    let item_type = image
        .get("itemType")
        .and_then(|value| value.get("S"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if event_name != INSERT || item_type != ItemType::Fact.id() {
        return Err(StreamError::Unfiltered {
            index,
            event_name: event_name.to_owned(),
            item_type: item_type.to_owned(),
            expected: ItemType::Fact.id(),
        });
    }

    // The key is read first and separately: a record whose image will not decode
    // still has to name the position it parks at, or the frontier could not be
    // quarantined and the poison would loop forever.
    let position = position_of(change);
    let fact = match from_stream_json(image).and_then(|map| {
        decode_fact(&map).map_err(|error| crate::attribute::RowError::Malformed {
            attribute: "NewImage",
            reason: error.to_string(),
        })
    }) {
        Ok(fact) => Ok(fact),
        Err(error) => Err(UndecodableRecord {
            workspace: position.clone().map(|(workspace, _)| workspace),
            sequence: position.and_then(|(_, sequence)| sequence),
            // Identifiers and a reason only. A quantity or a piece of evidence
            // copied into a park marker would put money evidence outside the
            // fact rows that own it.
            detail: error.to_string(),
        }),
    };
    Ok(StreamRecord { identifier, fact })
}

/// The workspace and sequence a record's key names, when the key itself decodes.
fn position_of(change: &serde_json::Value) -> Option<(WorkspaceId, Option<AcceptedSequence>)> {
    let keys = change.get("Keys")?;
    let text = |name: &str| {
        keys.get(name)
            .and_then(|value| value.get("S"))
            .and_then(serde_json::Value::as_str)
    };
    let workspace = WorkspaceId::parse(text("pk")?.strip_prefix(PARTITION_PREFIX)?).ok()?;
    let sequence = text("sk")
        .and_then(|sort| sort.strip_prefix(FACT_PREFIX))
        .and_then(|digits| digits.parse::<u64>().ok())
        .and_then(|value| AcceptedSequence::new(value).ok());
    Some((workspace, sequence))
}

/// One settlement receipt as this category's queue delivers it.
///
// TODO(cross-stream): `aex_internal_contracts::usage::SettlementReceipt` carries
// a meter rather than a category and neither the workspace nor the accepted
// sequence, so it cannot express what `ApplyReceipt` needs. This is the shape
// finance's receipt queue publishes today; folding the two together is X-2 in
// `references/rewrite/usage.md`.
/// The envelope is `deny_unknown_fields`: an unrecognised field means the
/// producer and the consumer disagree about what a receipt is, and guessing
/// which half is right over money is not a choice this consumer gets to make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReceiptEnvelope {
    /// Central's own receipt identity.
    pub receipt_id: String,
    /// The region the fact was measured in.
    pub region: String,
    /// The authority the fact belongs to.
    pub category: String,
    /// The fact this settles.
    pub fact_id: String,
    /// What central rated it at, in micro-USD. Never a floating-point value.
    pub rated_microusd: i128,
    /// The journal transaction the settlement posted under.
    pub transaction_id: String,
    /// The rate book central used.
    pub pricing_version: String,
    /// When central committed.
    pub settled_at: String,
    /// The workspace, when central carries it. Saves one index lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// The sequence, when central carries it. Saves one index lookup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted_sequence: Option<u64>,
}

/// Why a receipt message could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReceiptError {
    /// The envelope was not a queue batch.
    #[error("the event is not a `Records` batch")]
    NotABatch,
    /// A message was missing a field every SQS record carries.
    #[error("queue message {index} carries no `{field}`")]
    Incomplete {
        /// Which message.
        index: usize,
        /// What it was missing.
        field: &'static str,
    },
    /// The body did not parse.
    #[error("queue message `{identifier}` is not a settlement receipt: {reason}")]
    Malformed {
        /// Which message.
        identifier: String,
        /// Why it was refused.
        reason: String,
    },
    /// The receipt settles a sibling authority's fact.
    #[error("receipt `{identifier}` settles the `{category}` authority, not `{expected}`")]
    ForeignCategory {
        /// Which message.
        identifier: String,
        /// What it named.
        category: String,
        /// What this consumer serves.
        expected: &'static str,
    },
}

/// Reads one receipt batch.
///
/// # Errors
///
/// Returns [`ReceiptError`] for an envelope this consumer does not serve, for a
/// body that does not parse, and for a receipt addressed to a sibling authority.
/// All three are wiring defects: a receipt that belongs to another category was
/// delivered to the wrong queue, and settling it here would settle a sequence
/// this worker does not own.
pub fn receipt_records(event: &serde_json::Value) -> Result<Vec<ReceiptRecord>, ReceiptError> {
    let records = event
        .get("Records")
        .and_then(serde_json::Value::as_array)
        .ok_or(ReceiptError::NotABatch)?;
    let mut decoded = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        let identifier = record
            .get("messageId")
            .and_then(serde_json::Value::as_str)
            .ok_or(ReceiptError::Incomplete {
                index,
                field: "messageId",
            })?
            .to_owned();
        let body = record
            .get("body")
            .and_then(serde_json::Value::as_str)
            .ok_or(ReceiptError::Incomplete {
                index,
                field: "body",
            })?;
        let envelope: ReceiptEnvelope =
            serde_json::from_str(body).map_err(|error| ReceiptError::Malformed {
                identifier: identifier.clone(),
                reason: error.to_string(),
            })?;
        let receipt = settlement_receipt(&identifier, envelope)?;
        decoded.push(ReceiptRecord {
            identifier,
            receipt,
        });
    }
    Ok(decoded)
}

/// Turns one envelope into the port value, refusing a foreign authority.
fn settlement_receipt(
    identifier: &str,
    envelope: ReceiptEnvelope,
) -> Result<SettlementReceipt, ReceiptError> {
    if envelope.category != CATEGORY.id() {
        return Err(ReceiptError::ForeignCategory {
            identifier: identifier.to_owned(),
            category: envelope.category,
            expected: CATEGORY.id(),
        });
    }
    let refuse = |reason: String| ReceiptError::Malformed {
        identifier: identifier.to_owned(),
        reason,
    };
    Ok(SettlementReceipt {
        receipt_id: envelope.receipt_id,
        region: RegionId::parse(&envelope.region).map_err(|e| refuse(e.to_string()))?,
        category: CATEGORY,
        fact_id: FactId::parse(&envelope.fact_id).map_err(|e| refuse(e.to_string()))?,
        rated_microusd: envelope.rated_microusd,
        transaction_id: envelope.transaction_id,
        pricing_version: PricingVersion::parse(&envelope.pricing_version)
            .map_err(|e| refuse(e.to_string()))?,
        settled_at: Timestamp::parse(&envelope.settled_at).map_err(|e| refuse(e.to_string()))?,
        workspace: envelope
            .workspace_id
            .as_deref()
            .map(WorkspaceId::parse)
            .transpose()
            .map_err(|e| refuse(e.to_string()))?,
        accepted_sequence: envelope
            .accepted_sequence
            .map(AcceptedSequence::new)
            .transpose()
            .map_err(|e| refuse(e.to_string()))?,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        INSERT, ReceiptEnvelope, ReceiptError, StreamError, receipt_records, stream_records,
    };
    use crate::attribute;
    use crate::expressions::ComputeAuthority;
    use aex_usage_domain::fact::{
        Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
        UsageFact,
    };
    use aex_usage_domain::frontier::{AcceptedSequence, Frontier};
    use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use aex_usage_domain::interval::StorageClose;
    use aex_usage_domain::interval::storage::{StorageOwner, StorageOwnerKind, StorageSource};
    use aex_usage_domain::keys::ItemType;
    use aex_usage_domain::measurement::{
        BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ReservationClass, ServiceTime,
        SourceReceipt,
    };
    use aex_usage_domain::meter::{Category, Meter};
    use aex_usage_domain::wire_pending::{
        OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp, WorkspaceId,
    };
    use aws_sdk_dynamodb::types::AttributeValue;
    use std::collections::HashMap;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn region() -> RegionId {
        RegionId::parse("eu-west-1").expect("region")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    /// One measurement for the given authority, so this module is identical in
    /// all three adapters.
    fn measurement_for(category: Category) -> Measurement {
        match category {
            Category::Storage => Measurement::new(
                Meter::StorageByteMin,
                FactBasis::Consumed,
                ServiceTime::Interval {
                    start: at(0),
                    end: at(180_000),
                },
                SourceReceipt {
                    kind: ReceiptKind::StorageCommit,
                    id: Box::from("commit-1"),
                    digest: None,
                },
                Evidence::StorageResidence {
                    owner: StorageOwner {
                        kind: StorageOwnerKind::ContentObject,
                        id: AuthorityId::parse("obj-1").expect("id"),
                        generation: 1,
                    },
                    source: StorageSource::S3,
                    bytes: 4_096,
                    minutes: 3,
                    commit_id: Box::from("commit-1"),
                    close: StorageClose::InteriorFloor,
                },
            ),
            Category::Compute => Measurement::new(
                Meter::MemoryByteMs,
                FactBasis::Reserved,
                ServiceTime::Interval {
                    start: at(0),
                    end: at(250),
                },
                SourceReceipt {
                    kind: ReceiptKind::ReservationToken,
                    id: Box::from("res-1"),
                    digest: None,
                },
                Evidence::Reservation {
                    class: ReservationClass::Context,
                    bytes: 4_096,
                    held_ms: 250,
                },
            ),
            Category::Transfer => Measurement::new(
                Meter::DataTransferEgressByte,
                FactBasis::Consumed,
                ServiceTime::Instant { at: at(0) },
                SourceReceipt {
                    kind: ReceiptKind::DeliveryLog,
                    id: Box::from("d1"),
                    digest: None,
                },
                Evidence::DeliveryReceipt {
                    boundary: BoundaryId::CONTENT_DOWNLOAD,
                    receipt_id: Box::from("d1"),
                    bytes: 1_024,
                },
            ),
        }
        .expect("a valid measurement for its own meter")
    }

    fn authority_kind(category: Category) -> AuthorityKind {
        match category {
            Category::Storage => AuthorityKind::StorageResidence,
            Category::Compute => AuthorityKind::MemoryReservation,
            Category::Transfer => AuthorityKind::EgressCrossing,
        }
    }

    fn fact(sequence: u64) -> UsageFact {
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse("org-1").expect("org"),
            workspace: workspace(),
            region: region(),
            attribution: Attribution::default(),
            service: ServiceId::parse("regional-stream").expect("service"),
            resource: ResourceGeneration {
                kind: ResourceKind::MuxTask,
                generation: Box::from("task-1"),
            },
            authority: AuthorityKey {
                region: region(),
                category: crate::CATEGORY,
                kind: authority_kind(crate::CATEGORY),
                authority_id: AuthorityId::parse("a-1").expect("id"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("version"),
            reservation: None,
            kind: FactKind::Measured(measurement_for(crate::CATEGORY)),
        }
        .admit(
            AcceptedSequence::new(sequence).expect("positive"),
            at(600_000),
        )
        .expect("admits")
    }

    /// Renders a stored row the way the stream delivers it.
    fn as_image(row: &HashMap<String, AttributeValue>) -> serde_json::Value {
        let mut image = serde_json::Map::new();
        for (name, value) in row {
            let tagged = match value {
                AttributeValue::S(text) => serde_json::json!({ "S": text }),
                AttributeValue::N(number) => serde_json::json!({ "N": number }),
                AttributeValue::Bool(flag) => serde_json::json!({ "BOOL": flag }),
                other => panic!("an authority row writes no {other:?}"),
            };
            image.insert(name.clone(), tagged);
        }
        serde_json::Value::Object(image)
    }

    fn batch(image: &serde_json::Value, event_name: &str, sequence: u64) -> serde_json::Value {
        serde_json::json!({
            "Records": [{
                "eventID": "e1",
                "eventName": event_name,
                "eventSource": "aws:dynamodb",
                "dynamodb": {
                    "SequenceNumber": "000000000000000000001",
                    "Keys": {
                        "pk": { "S": "WS#ws-1" },
                        "sk": { "S": format!("FACT#{sequence:020}") }
                    },
                    "NewImage": image
                }
            }]
        })
    }

    fn fact_batch(original: &UsageFact) -> serde_json::Value {
        let row = attribute::item(&ComputeAuthority::new().fact_item(original).expect("builds"));
        batch(&as_image(&row), INSERT, original.accepted_sequence.get())
    }

    #[test]
    fn a_fact_insert_decodes_into_a_foldable_record() {
        let original = fact(7);
        let records = stream_records(&fact_batch(&original)).expect("reads");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].identifier, "000000000000000000001");
        assert_eq!(
            records[0].fact.as_ref().expect("decodes").fact_id,
            original.fact_id
        );
    }

    #[test]
    fn a_record_the_event_source_filter_should_have_excluded_fails_the_batch() {
        // Parking a customer's frontier for a misconfigured filter would blame
        // the wrong thing, so this is raised rather than quarantined.
        let original = fact(7);
        let row = attribute::item(
            &ComputeAuthority::new()
                .fact_item(&original)
                .expect("builds"),
        );
        for event_name in ["MODIFY", "REMOVE"] {
            assert!(matches!(
                stream_records(&batch(&as_image(&row), event_name, 7)),
                Err(StreamError::Unfiltered { .. })
            ));
        }

        let frontier = attribute::item(
            &ComputeAuthority::new()
                .frontier_item(&Frontier::empty(region(), workspace(), crate::CATEGORY))
                .expect("builds"),
        );
        assert!(matches!(
            stream_records(&batch(&as_image(&frontier), INSERT, 1)),
            Err(StreamError::Unfiltered { .. })
        ));
    }

    #[test]
    fn an_undecodable_image_parks_at_the_position_its_key_names() {
        // A permanent decode failure must be parkable, which means the record
        // has to name where it stopped even though its image is unreadable.
        let original = fact(7);
        let mut row = attribute::item(
            &ComputeAuthority::new()
                .fact_item(&original)
                .expect("builds"),
        );
        row.insert(
            crate::expressions::FACT_BODY.to_owned(),
            AttributeValue::S("{".to_owned()),
        );
        let records = stream_records(&batch(&as_image(&row), INSERT, 7)).expect("reads");
        let parked = records[0].fact.as_ref().expect_err("refuses");
        assert_eq!(parked.workspace.as_ref(), Some(&workspace()));
        assert_eq!(parked.sequence, AcceptedSequence::new(7).ok());
        assert!(!parked.detail.is_empty());
    }

    #[test]
    fn an_envelope_that_is_not_a_batch_is_refused() {
        assert!(matches!(
            stream_records(&serde_json::json!({})),
            Err(StreamError::NotABatch)
        ));
        assert!(matches!(
            stream_records(&serde_json::json!({ "Records": [{ "eventName": INSERT }] })),
            Err(StreamError::Incomplete { .. })
        ));
    }

    fn receipt_body(category: &str) -> String {
        serde_json::to_string(&ReceiptEnvelope {
            receipt_id: "rcpt-1".to_owned(),
            region: "eu-west-1".to_owned(),
            category: category.to_owned(),
            fact_id: fact(1).fact_id.to_string(),
            rated_microusd: -25,
            transaction_id: "txn-1".to_owned(),
            pricing_version: "synthetic-zero-v1".to_owned(),
            settled_at: at(700_000).to_canonical(),
            workspace_id: Some("ws-1".to_owned()),
            accepted_sequence: Some(1),
        })
        .expect("serializes")
    }

    fn receipt_batch(body: &str) -> serde_json::Value {
        serde_json::json!({
            "Records": [{ "messageId": "m1", "eventSource": "aws:sqs", "body": body }]
        })
    }

    #[test]
    fn a_receipt_message_decodes_including_a_reversal() {
        let records =
            receipt_records(&receipt_batch(&receipt_body(crate::CATEGORY.id()))).expect("reads");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].identifier, "m1");
        assert_eq!(records[0].receipt.rated_microusd, -25);
        assert_eq!(records[0].receipt.category, crate::CATEGORY);
        assert_eq!(records[0].receipt.workspace.as_ref(), Some(&workspace()));
        assert_eq!(
            records[0].receipt.accepted_sequence,
            AcceptedSequence::new(1).ok()
        );
    }

    #[test]
    fn a_receipt_for_a_sibling_authority_is_refused_never_settled_here() {
        // Settling it here would advance a sequence this worker does not own.
        for sibling in Category::ALL
            .into_iter()
            .filter(|candidate| *candidate != crate::CATEGORY)
        {
            assert!(matches!(
                receipt_records(&receipt_batch(&receipt_body(sibling.id()))),
                Err(ReceiptError::ForeignCategory { .. })
            ));
        }
    }

    #[test]
    fn a_receipt_body_the_producer_and_consumer_disagree_about_is_refused() {
        // `deny_unknown_fields`: guessing which half is right over money is not
        // a choice this consumer gets to make.
        let mut body: serde_json::Value =
            serde_json::from_str(&receipt_body(crate::CATEGORY.id())).expect("JSON");
        body["somethingNew"] = serde_json::Value::from(1);
        assert!(matches!(
            receipt_records(&receipt_batch(&body.to_string())),
            Err(ReceiptError::Malformed { .. })
        ));

        let mut missing: serde_json::Value =
            serde_json::from_str(&receipt_body(crate::CATEGORY.id())).expect("JSON");
        missing
            .as_object_mut()
            .expect("object")
            .remove("transactionId");
        assert!(matches!(
            receipt_records(&receipt_batch(&missing.to_string())),
            Err(ReceiptError::Malformed { .. })
        ));
    }

    #[test]
    fn the_item_type_this_stream_serves_is_the_one_the_table_declares() {
        assert_eq!(ItemType::Fact.id(), "usage_fact");
    }
}
