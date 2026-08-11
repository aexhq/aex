//! The regional outbox message, converted to the central settlement contract.
//!
//! This is the wire boundary. Finance owns `aex-<plane>-usage-rating.fifo`, its
//! `PRIMARY KEY (region, category, fact_id)` inbox, its `business_key` grammar
//! and its `intent_hash` rule — and the shape that crosses the queue is the one
//! declaration in [`aex_internal_contracts::usage::RatingRequest`], which the
//! settlement worker decodes through the same crate. This module converts the
//! regional domain fact into that contract; it declares no wire shape of its
//! own, because a second declaration is how the producer and the consumer once
//! came to serialize two different facts for one queue.
//!
//! The two identities that carry the money guarantee:
//!
//! - `MessageGroupId = organizationId`, so settlement is ordered per paying
//!   account (`OD-19`). Regional fan-out does not need global ordering; an
//!   account's ledger does.
//! - `MessageDeduplicationId = {region}:{category}:{factId}`, derived entirely
//!   from the deterministic fact identity, with `factId` in the contract's bare
//!   64-hex spelling (the regional `usage_` prefix stays regional — central
//!   keys its inbox on the bare digest, exactly as
//!   `aex_finance_app::use_cases::FifoRatingMessage` derives it). A producer
//!   retry, an SQS redelivery and a sweep republish therefore all collapse
//!   onto one inbox row with no extra state anywhere.
//!
//! What the contract does not carry — deliberately dropped at this boundary,
//! not silently lost: `service` travels as `source_receipt.source`,
//! `accepted_at` as `source_receipt.observed_at`; `resource`, `reservation`,
//! `attribution.agent`, the domain authority kind and the local
//! `accepted_sequence` stay regional, because central rates and settles without
//! them and re-reads the regional authority when it needs provenance.

use aex_internal_contracts::usage as contracts;
use aex_internal_contracts::usage::RatingRequest;
use aex_internal_contracts::{PricingVersion, RunId, SchemaVersion};
use aex_usage_domain::fact::{FactKind, UsageFact};
use aex_usage_domain::identity::FACT_ID_PREFIX;
use aex_usage_domain::measurement::Measurement;
use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{OrganizationId, PrefixedId, WorkspaceId};
use aex_wire::types::{DecimalU128, Region, Timestamp};

/// The `business_key` prefix every usage settlement posts under.
pub const BUSINESS_KEY_PREFIX: &str = "usage";

/// Why an outbox message could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    /// The fact kind has no representation in the central contract.
    ///
    /// [`contracts::UsageFact`] is flat — one meter, one basis, one quantity —
    /// so a correction (`replace`, `void`) or a zero-dollar observability count
    /// cannot cross this wire yet. No live producer constructs these kinds
    /// (every draft constructor outside test fixtures builds
    /// `FactKind::Measured`), so this refusal is unreachable on the money path
    /// today. The correction/observability producer, when it lands, owns
    /// extending `aex-internal-contracts::usage` and removing this refusal —
    /// flattening a correction into a plain measured fact would double-bill,
    /// and dropping it silently would lose money evidence. Until then the
    /// blast radius is bounded and loud: the stream worker quarantines the
    /// fact (parking its workspace visibly), and the sweep counts the row as
    /// `unpublishable` and steps over it rather than aborting the shard.
    #[error("a `{kind}` fact has no representation in the central rating contract")]
    Unrepresentable {
        /// The domain fact-kind discriminator.
        kind: &'static str,
    },
    /// A fact field could not be expressed in the contract's grammar.
    ///
    /// Live producers mint every identifier in the canonical `aex_wire` form
    /// (the Hands lifecycle path parses `FactContext` ids from their encoded
    /// spellings), so this refusal means a producer regressed to a local
    /// spelling — the honest outcome is a loud publish failure, not a message
    /// central cannot attribute.
    #[error("`{field}` cannot be expressed in the central contract: {reason}")]
    ForeignIdentity {
        /// The fact field that was refused.
        field: &'static str,
        /// Why it was refused.
        reason: String,
    },
}

/// One message as it is handed to the central FIFO queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxMessage {
    /// The FIFO ordering group: one paying account, one order.
    pub message_group_id: String,
    /// The FIFO dedupe identity, derived only from the fact identity.
    pub message_deduplication_id: String,
    /// The journal identity central posts the settlement under.
    pub business_key: String,
    /// The rate book pinned at admission; never `current`.
    pub pricing_version: String,
    /// The body, in the shape the settlement worker decodes.
    pub body: RatingRequest,
}

impl OutboxMessage {
    /// Builds the message for one admitted fact.
    ///
    /// # Errors
    ///
    /// Returns [`OutboxError::Unrepresentable`] for a fact kind the central
    /// contract cannot carry, and [`OutboxError::ForeignIdentity`] when an
    /// identifier, region or instant cannot be expressed in the contract's
    /// grammar.
    pub fn for_fact(fact: &UsageFact) -> Result<Self, OutboxError> {
        let contract = contract_fact(fact)?;
        let intent_hash = IntentDigest::from_bytes(*fact.idempotency.intent_hash.as_bytes());
        Ok(Self {
            message_group_id: contract.organization.encode().as_str().to_owned(),
            message_deduplication_id: contract.idempotency.deduplication_id.to_string(),
            business_key: contract.idempotency.business_key.to_string(),
            pricing_version: contract.pricing_version.0.clone(),
            body: RatingRequest {
                fact: contract,
                intent_hash,
            },
        })
    }

    /// The `(region, category, fact_id)` triple the central inbox keys on.
    #[must_use]
    pub fn inbox_key(&self) -> (Region, &'static str, String) {
        (
            self.body.fact.region,
            self.body.fact.meter.category(),
            self.body.fact.fact_id.to_string(),
        )
    }
}

/// Converts one admitted domain fact into the central contract's shape.
///
/// Every component of the FIFO and journal grammar is canonical by
/// construction here — the region is a closed enum, the category a declared
/// constant and the fact id lowercase hex — so no runtime grammar check
/// remains; a value that could violate the grammar is unrepresentable.
fn contract_fact(fact: &UsageFact) -> Result<contracts::UsageFact, OutboxError> {
    let measurement = match &fact.kind {
        FactKind::Measured(measurement) => measurement,
        other => {
            return Err(OutboxError::Unrepresentable { kind: other.id() });
        }
    };
    let region = Region::from_name(fact.region.as_str()).ok_or_else(|| {
        foreign(
            "region",
            format!("`{}` is not a contract region", fact.region),
        )
    })?;
    let organization: OrganizationId = parse_id("organization", fact.organization.as_str())?;
    let workspace: WorkspaceId = parse_id("workspace", fact.workspace.as_str())?;
    let fact_id = wire_fact_id(fact)?;
    let meter = wire_meter(measurement.meter());
    let identity = format!("{}:{}:{fact_id}", region.as_str(), meter.category());
    Ok(contracts::UsageFact {
        schema_version: SchemaVersion(u32::from(fact.schema_version.get())),
        fact_id,
        meter,
        organization,
        workspace,
        region,
        attribution: contracts::Attribution {
            session: parse_attributed("attribution.session", fact.attribution.session.as_ref())?,
            run: parse_internal_run("attribution.run", fact.attribution.run.as_ref())?,
            operation: parse_attributed(
                "attribution.operation",
                fact.attribution.operation.as_ref(),
            )?,
        },
        authority: contracts::FactAuthority {
            kind: wire_authority_kind(fact),
            authority_id: Box::from(fact.authority.authority_id.as_str()),
            segment_ordinal: DecimalU128::new(u128::from(fact.authority.segment_ordinal.get())),
        },
        basis: wire_basis(measurement),
        quantity: DecimalU128::new(measurement.quantity().get()),
        service_time: wire_service_time(measurement)?,
        source_receipt: contracts::SourceReceipt {
            source: Box::from(fact.service.as_str()),
            receipt_id: measurement.source_receipt().id.clone(),
            // The domain receipt carries no producer instant; the admission
            // instant is the closest authority-attested one a reconciler can
            // anchor on.
            observed_at: wire_timestamp("accepted_at", fact.accepted_at.unix_millis())?,
        },
        pricing_version: PricingVersion(fact.pricing_version.as_str().to_owned()),
        idempotency: contracts::FactIdempotency {
            deduplication_id: identity.clone().into_boxed_str(),
            business_key: format!("{BUSINESS_KEY_PREFIX}:{identity}").into_boxed_str(),
        },
    })
}

/// Transcodes the domain fact identity into the contract's bare digest.
fn wire_fact_id(fact: &UsageFact) -> Result<contracts::FactId, OutboxError> {
    let refuse = || {
        foreign(
            "fact_id",
            format!("`{}` is not a `usage_`-prefixed digest", fact.fact_id),
        )
    };
    let hex = fact
        .fact_id
        .as_str()
        .strip_prefix(FACT_ID_PREFIX)
        .ok_or_else(refuse)?;
    if hex.len() != 64 {
        return Err(refuse());
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).map_err(|_| refuse())?;
    }
    Ok(contracts::FactId::from_bytes(bytes))
}

/// The contract meter of one priced domain meter.
const fn wire_meter(meter: aex_usage_domain::meter::Meter) -> contracts::Meter {
    use aex_usage_domain::meter::Meter;
    match meter {
        Meter::ComputeMillicpuMs => contracts::Meter::ComputeMillicpuMs,
        Meter::MemoryByteMs => contracts::Meter::MemoryByteMs,
        Meter::StorageByteMin => contracts::Meter::StorageByteMin,
        Meter::DataTransferEgressByte => contracts::Meter::DataTransferEgressByte,
    }
}

/// The contract authority of one domain authority category.
const fn wire_authority_kind(fact: &UsageFact) -> contracts::AuthorityKind {
    use aex_usage_domain::meter::Category;
    match fact.category() {
        Category::Storage => contracts::AuthorityKind::Storage,
        Category::Compute => contracts::AuthorityKind::Compute,
        Category::Transfer => contracts::AuthorityKind::Transfer,
    }
}

/// The contract basis of one measurement.
const fn wire_basis(measurement: &Measurement) -> contracts::FactBasis {
    use aex_usage_domain::measurement::FactBasis;
    match measurement.basis() {
        FactBasis::Consumed => contracts::FactBasis::Consumed,
        FactBasis::Reserved => contracts::FactBasis::Reserved,
    }
}

/// The contract service time of one measurement.
fn wire_service_time(measurement: &Measurement) -> Result<contracts::ServiceTime, OutboxError> {
    use aex_usage_domain::measurement::ServiceTime;
    Ok(match measurement.service_time() {
        ServiceTime::Instant { at } => contracts::ServiceTime::Instant {
            at: wire_timestamp("service_time.at", at.unix_millis())?,
        },
        ServiceTime::Interval { start, end } => contracts::ServiceTime::Interval {
            start: wire_timestamp("service_time.start", start.unix_millis())?,
            end: wire_timestamp("service_time.end", end.unix_millis())?,
        },
    })
}

/// One contract instant from domain epoch milliseconds.
fn wire_timestamp(field: &'static str, millis: i64) -> Result<Timestamp, OutboxError> {
    Timestamp::from_unix_millis(millis).map_err(|error| foreign(field, error.to_string()))
}

/// Parses one required identifier into its canonical wire type.
fn parse_id<I: PrefixedId>(field: &'static str, value: &str) -> Result<I, OutboxError> {
    I::parse(value).map_err(|error| foreign(field, error.to_string()))
}

/// Parses one optional attribution identifier.
fn parse_attributed<I, D>(field: &'static str, value: Option<&D>) -> Result<Option<I>, OutboxError>
where
    I: PrefixedId,
    D: std::fmt::Display,
{
    value.map(|id| parse_id(field, &id.to_string())).transpose()
}

/// Parses one private execution identity without re-exposing it through the
/// public [`PrefixedId`] registry.
fn parse_internal_run<D>(
    field: &'static str,
    value: Option<&D>,
) -> Result<Option<RunId>, OutboxError>
where
    D: std::fmt::Display,
{
    value
        .map(|id| RunId::parse(&id.to_string()).map_err(|error| foreign(field, error.to_string())))
        .transpose()
}

/// Shapes one grammar refusal.
fn foreign(field: &'static str, reason: impl Into<String>) -> OutboxError {
    OutboxError::ForeignIdentity {
        field,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{BUSINESS_KEY_PREFIX, OutboxError, OutboxMessage};
    use crate::testing::{fact, observability_fact, organization, replace_fact, void_fact};
    use aex_usage_domain::meter::Category;
    use aex_wire::ids::PrefixedId as _;

    /// The bare 64-hex spelling of one domain fact identity.
    fn bare_hex(fact: &aex_usage_domain::fact::UsageFact) -> &str {
        fact.fact_id
            .as_str()
            .strip_prefix("usage_")
            .expect("a domain fact id carries the regional prefix")
    }

    #[test]
    fn the_dedupe_identity_is_the_central_grammar_exactly() {
        let fact = fact(Category::Storage, 1);
        let message = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(
            message.message_deduplication_id,
            format!("eu-west-1:storage:{}", bare_hex(&fact)),
            "finance keys its inbox on this triple in the bare digest spelling; \
             a different spelling is a second settlement, not a second name"
        );
        assert_eq!(
            message.business_key,
            format!("{BUSINESS_KEY_PREFIX}:{}", message.message_deduplication_id)
        );
    }

    #[test]
    fn the_fact_carries_its_own_message_identities_byte_for_byte() {
        // The attributes SQS sees and the idempotency the fact itself carries
        // must be the same bytes: central folds on the fact's copy, SQS dedupes
        // on the attribute, and a divergence is a duplicate settlement.
        let message = OutboxMessage::for_fact(&fact(Category::Transfer, 2)).expect("builds");
        assert_eq!(
            message.body.fact.idempotency.deduplication_id.as_ref(),
            message.message_deduplication_id
        );
        assert_eq!(
            message.body.fact.idempotency.business_key.as_ref(),
            message.business_key
        );
        assert_eq!(message.body.fact.pricing_version.0, message.pricing_version);
    }

    #[test]
    fn the_ordering_group_is_the_paying_account() {
        let message = OutboxMessage::for_fact(&fact(Category::Compute, 1)).expect("builds");
        assert_eq!(
            message.message_group_id,
            organization().as_str(),
            "settlement is ordered per organization, which is the money key"
        );
        assert_eq!(
            message.message_group_id,
            message.body.fact.organization.encode().as_str(),
            "the FIFO group and the fact's own account are one identity"
        );
    }

    #[test]
    fn a_producer_retry_produces_a_byte_identical_message() {
        // The whole idempotency guarantee rests on this: nothing in the message
        // is minted at send time, so a retry, a redelivery and a sweep
        // republish are the same message.
        let fact = fact(Category::Transfer, 4);
        let first = OutboxMessage::for_fact(&fact).expect("builds");
        let second = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first.body).expect("serializes"),
            serde_json::to_string(&second.body).expect("serializes")
        );
    }

    #[test]
    fn two_categories_of_one_workspace_never_share_a_dedupe_identity() {
        let mut identities: Vec<String> = Category::ALL
            .into_iter()
            .map(|category| {
                OutboxMessage::for_fact(&fact(category, 1))
                    .expect("builds")
                    .message_deduplication_id
            })
            .collect();
        let count = identities.len();
        identities.sort();
        identities.dedup();
        assert_eq!(count, identities.len());
    }

    #[test]
    fn the_inbox_key_is_the_triple_finance_declares() {
        let fact = fact(Category::Storage, 7);
        let (region, category, fact_id) =
            OutboxMessage::for_fact(&fact).expect("builds").inbox_key();
        assert_eq!(region.as_str(), fact.region.as_str());
        assert_eq!(category, "storage");
        assert_eq!(fact_id, bare_hex(&fact));
    }

    #[test]
    fn a_zero_quantity_measured_fact_is_still_published() {
        // Zero-dollar is not zero-evidence: a measured fact whose quantity is
        // zero still arrives, and central rates it to nothing.
        let fact = crate::testing::admit(
            crate::testing::draft(
                Category::Transfer,
                "zero-1",
                aex_usage_domain::fact::FactKind::Measured(crate::testing::measurement_for(
                    Category::Transfer,
                    0,
                )),
            ),
            1,
        );
        let message = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(message.body.fact.quantity.get(), 0);
        assert_eq!(
            message.body.intent_hash.as_bytes(),
            fact.idempotency.intent_hash.as_bytes()
        );
    }

    #[test]
    fn a_correction_and_an_observability_fact_are_refused_not_garbled() {
        // The central contract cannot carry a correction or an observability
        // count yet, and no live producer mints one. Publishing them in a shape
        // the settlement worker cannot decode — the pre-fix behaviour — poisons
        // the queue; flattening them into measured facts double-bills. The only
        // honest outcome until the contract grows is a typed refusal.
        let target = fact(Category::Compute, 1);
        for (fact, kind) in [
            (void_fact(Category::Compute, 2, &target.fact_id), "void"),
            (
                replace_fact(Category::Compute, 3, &target.fact_id, 10),
                "replace",
            ),
            (observability_fact(4), "observability"),
        ] {
            assert_eq!(
                OutboxMessage::for_fact(&fact),
                Err(OutboxError::Unrepresentable { kind }),
                "`{kind}` must refuse loudly"
            );
        }
    }

    #[test]
    fn the_body_carries_the_admission_intent_hash_not_a_recomputed_one() {
        // A recomputed hash would agree with itself even if the stored fact had
        // drifted, which is exactly the conflict the central inbox exists to
        // catch.
        let fact = fact(Category::Storage, 2);
        let message = OutboxMessage::for_fact(&fact).expect("builds");
        assert_eq!(
            message.body.intent_hash.as_bytes(),
            fact.idempotency.intent_hash.as_bytes()
        );
        assert_eq!(message.pricing_version, fact.pricing_version.as_str());
    }

    #[test]
    fn the_converted_fact_preserves_every_billing_field() {
        let fact = fact(Category::Compute, 5);
        let converted = OutboxMessage::for_fact(&fact).expect("builds").body.fact;
        let measurement = fact.kind.measurement().expect("a measured fixture");
        assert_eq!(converted.meter.as_str(), measurement.meter().id());
        assert_eq!(converted.quantity.get(), measurement.quantity().get());
        assert_eq!(
            converted.authority.authority_id.as_ref(),
            fact.authority.authority_id.as_str()
        );
        assert_eq!(
            converted.authority.segment_ordinal.get(),
            u128::from(fact.authority.segment_ordinal.get())
        );
        assert_eq!(converted.pricing_version.0, fact.pricing_version.as_str());
        assert_eq!(
            converted.source_receipt.source.as_ref(),
            fact.service.as_str()
        );
        assert_eq!(
            converted.source_receipt.observed_at.unix_millis(),
            fact.accepted_at.unix_millis()
        );
    }
}
