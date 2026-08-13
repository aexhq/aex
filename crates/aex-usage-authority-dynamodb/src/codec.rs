//! The strict read path: stored row to domain value.
//!
//! Every decode here is *checked redundancy*, not trust. A fact row carries both
//! the whole serialized fact and the flat attributes the indexes and operators
//! read, and [`decode_fact`] refuses a row whose two halves disagree. That is
//! deliberate: the cheapest way for money evidence to go wrong quietly is for a
//! read path to believe one half of a row and ignore the other.
//!
//! Three independent checks run on every fact read:
//!
//! 1. **Re-admission.** The decoded fact is taken apart into the draft a
//!    producer offered and re-admitted at its stored position. That re-runs the
//!    region, category, observability, service-time-skew and pinned-pricing
//!    fences, and recomputes both the deterministic identity and the intent
//!    hash. A row that would not be admitted today is not served today.
//! 2. **Re-derivation.** The measurement is rebuilt through
//!    [`Measurement::new`], which recomputes the quantity from the evidence. A
//!    quantity that no longer follows from its own evidence is refused.
//! 3. **Cross-check.** The flat attributes must agree with the body on every
//!    money-relevant field.
//!
//! All three are terminal failures. A row that fails one never passes it later,
//! so the caller quarantines rather than retrying.

use std::collections::HashMap;

use aex_usage_domain::fact::{FactDraft, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, FrontierState, PoisonReason};
use aex_usage_domain::identity::FactId;
use aex_usage_domain::keys::ItemType;
use aex_usage_domain::measurement::Measurement;
use aex_usage_domain::meter::Category;
use aex_usage_domain::wire_pending::{
    OrganizationId, PricingVersion, RegionId, Timestamp, WorkspaceId,
};
use aws_sdk_dynamodb::types::AttributeValue;

use crate::AuthorityBinding;
use crate::attribute::{Row, RowError};
use crate::expressions::{Authority, FACT_BODY};
use crate::gsi::OUTBOX_DUE_SORT as DUE_SORT;

/// Why a stored row could not be turned into a domain value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    /// An attribute was absent or the wrong shape.
    #[error(transparent)]
    Row(#[from] RowError),
    /// The serialized body did not parse.
    #[error("`{FACT_BODY}` is not a serialized usage fact: {reason}")]
    Body {
        /// Why it was refused.
        reason: String,
    },
    /// The stored fact would not be admitted today.
    #[error("stored fact `{fact_id}` no longer satisfies admission: {reason}")]
    Inadmissible {
        /// The fact that was refused.
        fact_id: String,
        /// Which fence refused it.
        reason: String,
    },
    /// The two halves of one row disagree.
    #[error("`{attribute}` is `{stored}` on the row but `{body}` in the fact body")]
    Drift {
        /// Which attribute disagreed.
        attribute: &'static str,
        /// What the flat attribute says.
        stored: String,
        /// What the body says.
        body: String,
    },
    /// A stored identifier or enumeration was not one this authority writes.
    #[error("`{what}` is not a value this authority writes: {reason}")]
    Unknown {
        /// Which field.
        what: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// Decodes one immutable fact row, checking every half against the others.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute,
/// [`DecodeError::Body`] for an unparseable body, [`DecodeError::Inadmissible`]
/// when re-admission or re-derivation refuses the stored fact, and
/// [`DecodeError::Drift`] when the flat attributes and the body disagree.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_fact<B: AuthorityBinding>(
    map: &HashMap<String, AttributeValue>,
) -> Result<UsageFact, DecodeError> {
    let row = Row::bind(map, ItemType::Fact)?;
    let stored: UsageFact =
        serde_json::from_str(row.string(FACT_BODY)?).map_err(|error| DecodeError::Body {
            reason: error.to_string(),
        })?;
    readmit(&stored)?;
    rederive(&stored)?;
    cross_check(row, &stored)?;

    // The category fence, one last time, on the way out. A sibling's meter in
    // this table is a table or IAM defect rather than a producer one, which is
    // why it is checked here as well as on the write path.
    let meter_id = stored
        .kind
        .meter()
        .map_or("observability", aex_usage_domain::meter::Meter::id);
    Authority::<B>::new()
        .verify_read(meter_id)
        .map_err(|error| DecodeError::Unknown {
            what: "meter",
            reason: error.to_string(),
        })?;
    Ok(stored)
}

/// Re-runs every admission fence over a stored fact.
///
/// Taking the fact back apart into the draft a producer offered and re-admitting
/// it at its stored position re-runs the region, category, observability,
/// service-time-skew and pinned-pricing checks, and recomputes both the
/// deterministic identity and the intent hash. A row that would not be admitted
/// today is not served today.
fn readmit(stored: &UsageFact) -> Result<(), DecodeError> {
    let draft = FactDraft {
        schema_version: stored.schema_version,
        organization: stored.organization.clone(),
        workspace: stored.workspace.clone(),
        region: stored.region.clone(),
        attribution: stored.attribution.clone(),
        service: stored.service.clone(),
        resource: stored.resource.clone(),
        authority: stored.authority.clone(),
        pricing_version: stored.pricing_version.clone(),
        reservation: stored.reservation.clone(),
        kind: stored.kind.clone(),
    };
    let readmitted = draft
        .admit(stored.accepted_sequence, stored.accepted_at)
        .map_err(|error| DecodeError::Inadmissible {
            fact_id: stored.fact_id.to_string(),
            reason: error.to_string(),
        })?;
    if &readmitted == stored {
        return Ok(());
    }
    Err(DecodeError::Inadmissible {
        fact_id: stored.fact_id.to_string(),
        reason: "the stored identity or intent hash does not follow from the stored fact"
            .to_owned(),
    })
}

/// Recomputes the stored quantity from the stored evidence.
///
/// `Deserialize` is derived on `Measurement`, so it bypasses the constructor
/// that derives the quantity. Rebuilding it here is what stops a body whose
/// quantity no longer follows from its own evidence.
fn rederive(stored: &UsageFact) -> Result<(), DecodeError> {
    let Some(measurement) = stored.kind.measurement() else {
        return Ok(());
    };
    let rebuilt = Measurement::new(
        measurement.meter(),
        measurement.basis(),
        measurement.service_time(),
        measurement.source_receipt().clone(),
        measurement.evidence().clone(),
    )
    .map_err(|error| DecodeError::Inadmissible {
        fact_id: stored.fact_id.to_string(),
        reason: error.to_string(),
    })?;
    if &rebuilt == measurement {
        return Ok(());
    }
    Err(DecodeError::Inadmissible {
        fact_id: stored.fact_id.to_string(),
        reason: "the stored quantity does not follow from the stored evidence".to_owned(),
    })
}

/// Refuses a row whose flat attributes and body disagree.
///
/// The flat attributes are what the two indexes and an operator read. A row
/// whose halves disagree is refused rather than resolved in favour of either
/// one: there is no way to tell which half drifted.
fn cross_check(row: Row<'_>, stored: &UsageFact) -> Result<(), DecodeError> {
    check("factId", row.string("factId")?, stored.fact_id.as_str())?;
    check(
        "workspaceId",
        row.string("workspaceId")?,
        stored.workspace.as_str(),
    )?;
    check(
        "organizationId",
        row.string("organizationId")?,
        stored.organization.as_str(),
    )?;
    check("region", row.string("region")?, stored.region.as_str())?;
    check(
        "acceptedSequence",
        &row.u64("acceptedSequence")?.to_string(),
        &stored.accepted_sequence.get().to_string(),
    )?;
    check(
        "acceptedAt",
        row.string("acceptedAt")?,
        &stored.accepted_at.to_canonical(),
    )?;
    check(
        "pricingVersion",
        row.string("pricingVersion")?,
        stored.pricing_version.as_str(),
    )?;
    check(
        "intentHash",
        row.string("intentHash")?,
        &stored.idempotency.intent_hash.to_string(),
    )?;
    check("factKind", row.string("factKind")?, stored.kind.id())?;

    match stored.kind.measurement() {
        Some(measurement) => {
            check("meter", row.string("meter")?, measurement.meter().id())?;
            check(
                "quantity",
                &row.u128("quantity")?.to_string(),
                &measurement.quantity().get().to_string(),
            )
        }
        None if row.has("quantity") => Err(DecodeError::Drift {
            attribute: "quantity",
            stored: row.u128("quantity")?.to_string(),
            body: "the fact carries no measurement".to_owned(),
        }),
        None => Ok(()),
    }
}

/// Decodes one frontier row.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute and
/// [`DecodeError::Unknown`] for a category, state or poison reason this
/// authority does not write.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_frontier<B: AuthorityBinding>(
    map: &HashMap<String, AttributeValue>,
) -> Result<Frontier, DecodeError> {
    let row = Row::bind(map, ItemType::Frontier)?;
    let category = category(row.string("category")?)?;
    if category != B::CATEGORY {
        return Err(DecodeError::Unknown {
            what: "category",
            reason: format!(
                "frontier declares `{}` but this adapter addresses `{}`",
                category.id(),
                B::CATEGORY.id()
            ),
        });
    }
    let state = match row.string("state")? {
        "advancing" => FrontierState::Advancing,
        "quarantined" => FrontierState::Quarantined {
            at: sequence("quarantineAt", row.u64("quarantineAt")?)?,
            reason: poison_reason(row.string("quarantineReason")?)?,
        },
        other => {
            return Err(DecodeError::Unknown {
                what: "state",
                reason: format!("`{other}` is not a frontier state"),
            });
        }
    };
    Ok(Frontier {
        region: identifier("region", row.string("region")?, RegionId::parse)?,
        workspace: identifier(
            "workspaceId",
            row.string("workspaceId")?,
            WorkspaceId::parse,
        )?,
        category,
        accepted: origin_or(row.u64("acceptedSequence")?, "acceptedSequence")?,
        projected: origin_or(row.u64("projectedSequence")?, "projectedSequence")?,
        published: origin_or(row.u64("publishedSequence")?, "publishedSequence")?,
        settled: origin_or(row.u64("settledSequence")?, "settledSequence")?,
        service_through: row.optional_timestamp("serviceThrough")?,
        state,
    })
}

/// One outbox row as the due index projects it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    /// The partition the fact lives in.
    pub workspace: WorkspaceId,
    /// The paying account.
    pub organization: OrganizationId,
    /// The region.
    pub region: RegionId,
    /// The fact awaiting delivery.
    pub fact_id: FactId,
    /// Its position in the sequence.
    pub accepted_sequence: AcceptedSequence,
    /// When it was first enqueued.
    pub enqueued_at: Timestamp,
    /// How many delivery attempts have been made.
    pub attempts: u32,
}

/// Decodes one outbox row, from the base table or the due index.
///
/// Both this authority's indexes project `itemType`, so the discriminator is
/// checked either way.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute and
/// [`DecodeError::Unknown`] for an identifier or category this authority does
/// not write.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_outbox<B: AuthorityBinding>(
    map: &HashMap<String, AttributeValue>,
) -> Result<OutboxRow, DecodeError> {
    let row = Row::bind(map, ItemType::Outbox)?;
    let category = category(row.string("category")?)?;
    if category != B::CATEGORY {
        return Err(DecodeError::Unknown {
            what: "category",
            reason: format!(
                "outbox row declares `{}` but this adapter addresses `{}`",
                category.id(),
                B::CATEGORY.id()
            ),
        });
    }
    Ok(OutboxRow {
        workspace: identifier(
            "workspaceId",
            row.string("workspaceId")?,
            WorkspaceId::parse,
        )?,
        organization: identifier(
            "organizationId",
            row.string("organizationId")?,
            OrganizationId::parse,
        )?,
        region: identifier("region", row.string("region")?, RegionId::parse)?,
        fact_id: identifier("factId", row.string("factId")?, FactId::parse)?,
        accepted_sequence: sequence("acceptedSequence", row.u64("acceptedSequence")?)?,
        enqueued_at: row.timestamp("enqueuedAt")?,
        attempts: row.u32("attempts")?,
    })
}

/// Decodes one outbox row as `gsi_outbox_due` projects it.
///
/// The index is an `INCLUDE` projection and does **not** carry `enqueuedAt`; it
/// carries `outDueSk`, which is `{enqueuedAt}#{sequence:020}` and is a key
/// attribute, so it is always present. The instant is read from there rather
/// than from an attribute the index does not project — reading a base-table
/// attribute off an index result is how a sweep decodes every row as corrupt.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute and
/// [`DecodeError::Unknown`] for an identifier, category or sort key this
/// authority does not write.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_due<B: AuthorityBinding>(
    map: &HashMap<String, AttributeValue>,
) -> Result<OutboxRow, DecodeError> {
    let row = Row::bind(map, ItemType::Outbox)?;
    let category = category(row.string("category")?)?;
    if category != B::CATEGORY {
        return Err(DecodeError::Unknown {
            what: "category",
            reason: format!(
                "outbox row declares `{}` but this adapter addresses `{}`",
                category.id(),
                B::CATEGORY.id()
            ),
        });
    }
    let sort = row.string(DUE_SORT)?;
    let (instant, _) = sort.split_once('#').ok_or_else(|| DecodeError::Unknown {
        what: DUE_SORT,
        reason: format!("`{sort}` is not `{{enqueuedAt}}#{{sequence}}`"),
    })?;
    let enqueued_at = Timestamp::parse(instant).map_err(|error| DecodeError::Unknown {
        what: DUE_SORT,
        reason: error.to_string(),
    })?;
    Ok(OutboxRow {
        workspace: identifier(
            "workspaceId",
            row.string("workspaceId")?,
            WorkspaceId::parse,
        )?,
        organization: identifier(
            "organizationId",
            row.string("organizationId")?,
            OrganizationId::parse,
        )?,
        region: identifier("region", row.string("region")?, RegionId::parse)?,
        fact_id: identifier("factId", row.string("factId")?, FactId::parse)?,
        accepted_sequence: sequence("acceptedSequence", row.u64("acceptedSequence")?)?,
        enqueued_at,
        attempts: row.u32("attempts")?,
    })
}

/// Where one identity claim says its fact lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRow {
    /// The partition.
    pub workspace: WorkspaceId,
    /// The position.
    pub accepted_sequence: AcceptedSequence,
    /// The intent hash admission recorded, which decides replay from conflict.
    pub intent_hash: String,
}

/// Decodes one identity claim, from the base table or `gsi_fact_id`.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_claim(map: &HashMap<String, AttributeValue>) -> Result<ClaimRow, DecodeError> {
    let row = Row::bind(map, ItemType::FactClaim)?;
    Ok(ClaimRow {
        workspace: identifier(
            "workspaceId",
            row.string("workspaceId")?,
            WorkspaceId::parse,
        )?,
        accepted_sequence: sequence("acceptedSequence", row.u64("acceptedSequence")?)?,
        intent_hash: row.string("intentHash")?.to_owned(),
    })
}

/// What one stored settlement receipt records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReceipt {
    /// Central's own receipt identity.
    pub receipt_id: String,
    /// What central rated the fact at, in micro-USD.
    pub rated_microusd: i128,
    /// The rate book central used.
    pub pricing_version: PricingVersion,
    /// When central committed.
    pub settled_at: Timestamp,
}

/// Decodes one settlement receipt row.
///
/// # Errors
///
/// Returns [`DecodeError::Row`] for a missing or mis-shaped attribute and
/// [`DecodeError::Unknown`] for an unparseable pricing version.
#[allow(
    clippy::implicit_hasher,
    reason = "the SDK hands over exactly this map type; generalizing buys nothing"
)]
pub fn decode_receipt(map: &HashMap<String, AttributeValue>) -> Result<StoredReceipt, DecodeError> {
    let row = Row::bind(map, ItemType::SettlementReceipt)?;
    Ok(StoredReceipt {
        receipt_id: row.string("receiptId")?.to_owned(),
        // Money may be negative when a correction reverses.
        rated_microusd: row.i128("ratedMicrousd")?,
        pricing_version: identifier(
            "pricingVersion",
            row.string("pricingVersion")?,
            PricingVersion::parse,
        )?,
        settled_at: row.timestamp("settledAt")?,
    })
}

/// Refuses a flat attribute that disagrees with the body.
fn check(attribute: &'static str, stored: &str, body: &str) -> Result<(), DecodeError> {
    if stored == body {
        return Ok(());
    }
    Err(DecodeError::Drift {
        attribute,
        stored: stored.to_owned(),
        body: body.to_owned(),
    })
}

/// Parses a stored identifier through its own grammar.
fn identifier<T, E: std::fmt::Display>(
    what: &'static str,
    value: &str,
    parse: impl Fn(&str) -> Result<T, E>,
) -> Result<T, DecodeError> {
    parse(value).map_err(|error| DecodeError::Unknown {
        what,
        reason: error.to_string(),
    })
}

/// Parses a stored category identifier.
fn category(value: &str) -> Result<Category, DecodeError> {
    Category::ALL
        .into_iter()
        .find(|candidate| candidate.id() == value)
        .ok_or_else(|| DecodeError::Unknown {
            what: "category",
            reason: format!("`{value}` is not an authority category"),
        })
}

/// Parses a stored poison reason.
fn poison_reason(value: &str) -> Result<PoisonReason, DecodeError> {
    PoisonReason::ALL
        .into_iter()
        .find(|candidate| candidate.id() == value)
        .ok_or_else(|| DecodeError::Unknown {
            what: "quarantineReason",
            reason: format!("`{value}` is not a poison reason"),
        })
}

/// A one-based position, refusing zero.
fn sequence(what: &'static str, value: u64) -> Result<AcceptedSequence, DecodeError> {
    AcceptedSequence::new(value).map_err(|error| DecodeError::Unknown {
        what,
        reason: error.to_string(),
    })
}

/// A frontier stage, where zero is the empty state rather than a position.
fn origin_or(value: u64, what: &'static str) -> Result<AcceptedSequence, DecodeError> {
    if value == 0 {
        return Ok(AcceptedSequence::ORIGIN);
    }
    sequence(what, value)
}

#[cfg(test)]
mod tests {
    use super::{
        DecodeError, OutboxRow, decode_claim, decode_due as decode_due_for,
        decode_fact as decode_fact_for, decode_frontier as decode_frontier_for,
        decode_outbox as decode_outbox_for, decode_receipt,
    };
    use crate::TestBinding;
    use crate::attribute::{self, from_stream_json};
    use crate::expressions::{Authority, FACT_BODY, ReceiptRow};
    use aex_usage_domain::fact::{
        Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
        UsageFact,
    };
    use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
    use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use aex_usage_domain::interval::StorageClose;
    use aex_usage_domain::interval::storage::{StorageOwner, StorageOwnerKind, StorageSource};
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

    type TestAuthority = Authority<TestBinding>;

    fn decode_fact(map: &HashMap<String, AttributeValue>) -> Result<UsageFact, DecodeError> {
        decode_fact_for::<TestBinding>(map)
    }

    fn decode_frontier(map: &HashMap<String, AttributeValue>) -> Result<Frontier, DecodeError> {
        decode_frontier_for::<TestBinding>(map)
    }

    fn decode_outbox(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        decode_outbox_for::<TestBinding>(map)
    }

    fn decode_due(map: &HashMap<String, AttributeValue>) -> Result<OutboxRow, DecodeError> {
        decode_due_for::<TestBinding>(map)
    }

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    fn workspace() -> WorkspaceId {
        WorkspaceId::parse("ws-1").expect("workspace")
    }

    fn region() -> RegionId {
        RegionId::parse("eu-west-1").expect("region")
    }

    /// One measurement for the given authority.
    ///
    /// Each category has one evidence shape used here, so this test module is
    /// identical in all three adapters and only the crate's own `CATEGORY`
    /// decides which arm runs.
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
                category: Category::Compute,
                kind: authority_kind(Category::Compute),
                authority_id: AuthorityId::parse("a-1").expect("id"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("version"),
            reservation: None,
            kind: FactKind::Measured(measurement_for(Category::Compute)),
        }
        .admit(
            AcceptedSequence::new(sequence).expect("positive"),
            at(600_000),
        )
        .expect("admits")
    }

    fn fact_row(fact: &UsageFact) -> HashMap<String, AttributeValue> {
        attribute::item(&TestAuthority::new().fact_item(fact).expect("builds"))
    }

    /// Renders a stored row the way the stream delivers it.
    fn as_stream_image(row: &HashMap<String, AttributeValue>) -> serde_json::Value {
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

    fn body_of(row: &HashMap<String, AttributeValue>) -> serde_json::Value {
        serde_json::from_str(row[FACT_BODY].as_s().expect("the body is a string"))
            .expect("the body is JSON")
    }

    #[test]
    fn a_written_fact_row_decodes_back_to_the_same_fact() {
        let original = fact(7);
        let decoded = decode_fact(&fact_row(&original)).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn the_stream_image_decodes_through_the_identical_path() {
        // A stream image that decoded through a laxer path would let a record
        // into the fold that a table read would have refused.
        let original = fact(3);
        let row = fact_row(&original);
        let image = from_stream_json(&as_stream_image(&row)).expect("converts");
        assert_eq!(decode_fact(&image).expect("decodes"), original);
    }

    #[test]
    fn a_row_whose_two_halves_disagree_is_refused_rather_than_resolved() {
        // There is no way to tell which half drifted, so neither is believed.
        for (attribute_name, replacement) in [
            (
                "factId",
                AttributeValue::S("usage_something_else".to_owned()),
            ),
            ("quantity", AttributeValue::N("999999".to_owned())),
            ("acceptedSequence", AttributeValue::N("8".to_owned())),
            ("intentHash", AttributeValue::S("blake3:0".to_owned())),
            ("meter", AttributeValue::S("not.a.meter.v1".to_owned())),
            (
                "pricingVersion",
                AttributeValue::S("synthetic-one-v1".to_owned()),
            ),
        ] {
            let mut row = fact_row(&fact(7));
            row.insert(attribute_name.to_owned(), replacement);
            assert!(
                matches!(decode_fact(&row), Err(DecodeError::Drift { .. })),
                "{attribute_name} drifted without being noticed"
            );
        }
    }

    #[test]
    fn a_body_whose_quantity_no_longer_follows_from_its_evidence_is_refused() {
        // `Deserialize` is derived on `Measurement`, so it would happily accept a
        // quantity nothing derived. The re-derivation is what catches it.
        let mut row = fact_row(&fact(7));
        let mut body = body_of(&row);
        assert!(
            body["kind"]["quantity"].is_string(),
            "the body must carry the derived quantity to be able to drift"
        );
        body["kind"]["quantity"] = serde_json::Value::String("1".to_owned());
        row.insert(FACT_BODY.to_owned(), AttributeValue::S(body.to_string()));
        assert!(matches!(
            decode_fact(&row),
            Err(DecodeError::Inadmissible { .. })
        ));
    }

    #[test]
    fn a_body_whose_identity_was_forged_is_refused() {
        // A well-formed identity that is not the one the authority key derives.
        // The grammar cannot catch this; only recomputing the identity can.
        let forged = format!("usage_{}", "a".repeat(64));
        let mut row = fact_row(&fact(7));
        let mut body = body_of(&row);
        body["fact_id"] = serde_json::Value::String(forged);
        row.insert(FACT_BODY.to_owned(), AttributeValue::S(body.to_string()));
        assert!(matches!(
            decode_fact(&row),
            Err(DecodeError::Inadmissible { .. })
        ));

        // An identity that is not even well formed never reaches that check.
        let mut row = fact_row(&fact(7));
        let mut body = body_of(&row);
        body["fact_id"] = serde_json::Value::String("not-an-identity".to_owned());
        row.insert(FACT_BODY.to_owned(), AttributeValue::S(body.to_string()));
        assert!(matches!(decode_fact(&row), Err(DecodeError::Body { .. })));
    }

    #[test]
    fn a_row_with_no_body_or_an_unparseable_one_is_refused() {
        let mut missing = fact_row(&fact(7));
        missing.remove(FACT_BODY);
        assert!(matches!(decode_fact(&missing), Err(DecodeError::Row(_))));

        let mut garbage = fact_row(&fact(7));
        garbage.insert(FACT_BODY.to_owned(), AttributeValue::S("{".to_owned()));
        assert!(matches!(
            decode_fact(&garbage),
            Err(DecodeError::Body { .. })
        ));
    }

    #[test]
    fn a_row_of_another_shape_is_never_decoded_as_a_fact() {
        let frontier = attribute::item(
            &TestAuthority::new()
                .frontier_item(&Frontier::empty(region(), workspace(), Category::Compute))
                .expect("builds"),
        );
        assert!(matches!(decode_fact(&frontier), Err(DecodeError::Row(_))));
    }

    #[test]
    fn a_frontier_round_trips_including_where_and_why_it_parked() {
        let advancing = Frontier::empty(region(), workspace(), Category::Compute)
            .admit(AcceptedSequence::new(1).expect("one"))
            .expect("admits");
        let row = attribute::item(
            &TestAuthority::new()
                .frontier_item(&advancing)
                .expect("builds"),
        );
        assert_eq!(decode_frontier(&row).expect("decodes"), advancing);

        let parked = advancing.quarantine(
            AcceptedSequence::new(1).expect("one"),
            PoisonReason::Undecodable,
        );
        let row = attribute::item(&TestAuthority::new().frontier_item(&parked).expect("builds"));
        assert_eq!(decode_frontier(&row).expect("decodes"), parked);
    }

    #[test]
    fn an_outbox_row_and_its_claim_round_trip() {
        let original = fact(9);
        let outbox = attribute::item(
            &TestAuthority::new()
                .outbox_item(&original, 5)
                .expect("builds"),
        );
        let decoded = decode_outbox(&outbox).expect("decodes");
        assert_eq!(decoded.fact_id, original.fact_id);
        assert_eq!(decoded.accepted_sequence, original.accepted_sequence);
        assert_eq!(decoded.workspace, original.workspace);
        assert_eq!(decoded.attempts, 0);

        let claim = attribute::item(&TestAuthority::new().claim_item(&original).expect("builds"));
        let decoded = decode_claim(&claim).expect("decodes");
        assert_eq!(decoded.workspace, original.workspace);
        assert_eq!(decoded.accepted_sequence, original.accepted_sequence);
        assert_eq!(
            decoded.intent_hash,
            original.idempotency.intent_hash.to_string()
        );
    }

    #[test]
    fn the_due_index_projection_decodes_where_the_base_row_decode_cannot() {
        // `gsi_outbox_due` is an INCLUDE projection and carries no `enqueuedAt`.
        // Reading a base-table attribute off an index result is how a sweep
        // decodes every row as corrupt, so the two paths are separate and each
        // is strict about what its own source projects.
        let original = fact(9);
        let full = attribute::item(
            &TestAuthority::new()
                .outbox_item(&original, 5)
                .expect("builds"),
        );
        // Exactly what the definition declares, plus the key attributes DynamoDB
        // always carries into a global secondary index.
        let projected: HashMap<String, AttributeValue> = [
            "itemType",
            "sk",
            "factId",
            "organizationId",
            "workspaceId",
            "region",
            "category",
            "acceptedSequence",
            "attempts",
            "pk",
            crate::gsi::OUTBOX_DUE_PARTITION,
            crate::gsi::OUTBOX_DUE_SORT,
        ]
        .into_iter()
        .map(|name| {
            (
                name.to_owned(),
                full.get(name)
                    .unwrap_or_else(|| panic!("the outbox row writes `{name}`"))
                    .clone(),
            )
        })
        .collect();

        assert!(
            decode_outbox(&projected).is_err(),
            "the base-table decode must not silently succeed on an index row"
        );
        let decoded = decode_due(&projected).expect("decodes");
        assert_eq!(decoded, decode_outbox(&full).expect("decodes"));
        assert_eq!(decoded.enqueued_at, original.accepted_at);
    }

    #[test]
    fn a_settlement_receipt_round_trips_including_a_reversal() {
        let original = fact(4);
        for rated in [1_234_i128, 0, -9_999] {
            let row = attribute::item(
                &TestAuthority::new()
                    .receipt_item(
                        &ReceiptRow {
                            receipt_id: "rcpt-1",
                            region: &region(),
                            category: Category::Compute,
                            fact_id: &original.fact_id,
                            rated_microusd: rated,
                            transaction_id: "txn-1",
                            pricing_version: &original.pricing_version,
                            settled_at: at(700_000),
                        },
                        &original.workspace,
                        original.accepted_sequence,
                    )
                    .expect("builds"),
            );
            let decoded = decode_receipt(&row).expect("decodes");
            assert_eq!(decoded.rated_microusd, rated, "a reversal must survive");
            assert_eq!(decoded.receipt_id, "rcpt-1");
            assert_eq!(decoded.settled_at, at(700_000));
        }
    }
}
