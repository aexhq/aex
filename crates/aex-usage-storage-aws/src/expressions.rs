//! Table expressions and the row codec for `usage-storage-authority`.
//!
//! Every path through this module is bound to [`crate::CATEGORY`]. A fact whose
//! meter belongs to a sibling authority is refused with
//! [`StoreError::CategoryEscape`] before any write is built, and refused again
//! when a row is read back. Two checks rather than one because the write side
//! and the read side fail differently: a bad write is a producer defect, a bad
//! read is a table or IAM defect, and conflating them would hide whichever came
//! second.

use std::collections::BTreeMap;

use aex_usage_domain::fact::{FactKind, UsageFact};
use aex_usage_domain::frontier::{Frontier, FrontierState};
use aex_usage_domain::keys::{AuthorityKeys, Item, ItemType, ItemValue, KeyError, padded};
use aex_usage_domain::meter::{Category, Meter};

use crate::CATEGORY;

/// Why a row could not be built or read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// A fact belonging to a sibling authority reached this adapter.
    ///
    /// A wiring or IAM defect, never a retryable condition: the fact is money
    /// evidence and belongs in a different table, so writing it here would put
    /// it beyond the reach of its own worker's frontier.
    #[error("meter `{meter}` belongs to the `{meter_category}` authority, not `{category}`")]
    CategoryEscape {
        /// The meter that was offered.
        meter: &'static str,
        /// Where that meter's facts belong.
        meter_category: &'static str,
        /// The authority this adapter addresses.
        category: &'static str,
    },
    /// A key could not be built.
    #[error(transparent)]
    Key(#[from] KeyError),
}

/// One conditional write inside a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactItem {
    /// The row being written.
    pub item: Item,
    /// The condition the write is guarded by.
    pub condition: &'static str,
}

/// The four-item single-partition admission transaction.
///
/// One commit boundary and one condition set: the identity claim fences a
/// duplicate, the fact row fences a rewrite, the frontier compare-and-set fences
/// a sequence collision, and the outbox row gives the sweep a durable "not yet
/// delivered" marker. All four share `WS#{workspace}`, so this is a
/// single-partition transaction rather than a cross-partition one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionTransaction {
    /// The `ID#{fact_id}` claim.
    pub claim: TransactItem,
    /// The `FACT#{sequence}` row.
    pub fact: TransactItem,
    /// The `FRONTIER` compare-and-set.
    pub frontier: TransactItem,
    /// The `OUTBOX#{sequence}` marker.
    pub outbox: TransactItem,
}

impl AdmissionTransaction {
    /// The four items in commit order.
    #[must_use]
    pub fn items(&self) -> [&TransactItem; 4] {
        [&self.claim, &self.fact, &self.frontier, &self.outbox]
    }
}

/// The frontier compare-and-set: the sequence must be exactly the one this
/// admission expects, so two concurrent producers cannot both claim it.
pub const FRONTIER_CAS: &str =
    "attribute_not_exists(pk) OR acceptedSequence = :previousAcceptedSequence";

/// The condition every write-once row carries.
pub const WRITE_ONCE: &str = "attribute_not_exists(pk)";

/// The `usage-storage-authority` expression builder.
#[derive(Debug, Clone, Copy)]
pub struct StorageAuthority {
    keys: AuthorityKeys,
}

impl Default for StorageAuthority {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageAuthority {
    /// Binds the builder to this crate's one category.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            keys: AuthorityKeys::new(CATEGORY),
        }
    }

    /// The authority this builder addresses.
    #[must_use]
    pub const fn category(self) -> Category {
        CATEGORY
    }

    /// Refuses a fact whose meter belongs to a sibling authority.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CategoryEscape`] for any meter outside
    /// [`crate::CATEGORY`].
    pub fn guard(self, fact: &UsageFact) -> Result<(), StoreError> {
        // An observability fact carries no priced meter, so it is placed by its
        // authority key. A priced fact must agree with its key too, which
        // `FactDraft::admit` already enforced; this re-checks at the table edge.
        let category = fact.kind.meter().map_or(fact.category(), Meter::category);
        if category == CATEGORY && fact.category() == CATEGORY {
            return Ok(());
        }
        Err(StoreError::CategoryEscape {
            meter: fact.kind.meter().map_or("observability", Meter::id),
            meter_category: category.id(),
            category: CATEGORY.id(),
        })
    }

    /// The immutable `FACT#` row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CategoryEscape`] for a foreign meter and
    /// [`StoreError::Key`] when the key cannot be built.
    pub fn fact_item(self, fact: &UsageFact) -> Result<Item, StoreError> {
        self.guard(fact)?;
        let key = self.keys.fact(&fact.workspace, fact.accepted_sequence)?;
        Ok(Item::new(
            CATEGORY,
            key,
            ItemType::Fact,
            fact_attributes(fact),
        )?)
    }

    /// The `ID#` claim that makes the deterministic identity a fence.
    ///
    /// A GSI cannot be condition-checked, which is why this claim exists as its
    /// own item rather than as an index over the fact row.
    ///
    /// # Errors
    ///
    /// As [`StorageAuthority::fact_item`].
    pub fn claim_item(self, fact: &UsageFact) -> Result<Item, StoreError> {
        self.guard(fact)?;
        let key = self.keys.claim(&fact.workspace, &fact.fact_id)?;
        let attributes = BTreeMap::from([
            (
                "factId".to_owned(),
                ItemValue::text(fact.fact_id.to_string()),
            ),
            (
                "acceptedSequence".to_owned(),
                ItemValue::number(fact.accepted_sequence.get()),
            ),
            (
                "intentHash".to_owned(),
                ItemValue::text(fact.idempotency.intent_hash.to_string()),
            ),
            (
                "organizationId".to_owned(),
                ItemValue::text(fact.organization.to_string()),
            ),
            (
                "workspaceId".to_owned(),
                ItemValue::text(fact.workspace.to_string()),
            ),
        ]);
        Ok(Item::new(CATEGORY, key, ItemType::FactClaim, attributes)?)
    }

    /// The `FRONTIER` position row.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CategoryEscape`] when the frontier belongs to a
    /// sibling authority, and [`StoreError::Key`] when the key cannot be built.
    pub fn frontier_item(self, frontier: &Frontier) -> Result<Item, StoreError> {
        if frontier.category != CATEGORY {
            return Err(StoreError::CategoryEscape {
                meter: "frontier",
                meter_category: frontier.category.id(),
                category: CATEGORY.id(),
            });
        }
        let key = self.keys.frontier(&frontier.workspace)?;
        let mut attributes = BTreeMap::from([
            ("category".to_owned(), ItemValue::text(CATEGORY.id())),
            (
                "region".to_owned(),
                ItemValue::text(frontier.region.to_string()),
            ),
            (
                "workspaceId".to_owned(),
                ItemValue::text(frontier.workspace.to_string()),
            ),
            (
                "acceptedSequence".to_owned(),
                ItemValue::number(frontier.accepted.get()),
            ),
            (
                "projectedSequence".to_owned(),
                ItemValue::number(frontier.projected.get()),
            ),
            (
                "publishedSequence".to_owned(),
                ItemValue::number(frontier.published.get()),
            ),
            (
                "settledSequence".to_owned(),
                ItemValue::number(frontier.settled.get()),
            ),
        ]);
        if let Some(through) = frontier.service_through {
            attributes.insert("serviceThrough".to_owned(), ItemValue::instant(through));
        }
        match frontier.state {
            FrontierState::Advancing => {
                attributes.insert("state".to_owned(), ItemValue::text("advancing"));
            }
            FrontierState::Quarantined { at, reason } => {
                attributes.insert("state".to_owned(), ItemValue::text("quarantined"));
                attributes.insert("quarantineAt".to_owned(), ItemValue::number(at.get()));
                attributes.insert("quarantineReason".to_owned(), ItemValue::text(reason.id()));
            }
        }
        Ok(Item::new(CATEGORY, key, ItemType::Frontier, attributes)?)
    }

    /// The `OUTBOX#` marker the sweep reads.
    ///
    /// # Errors
    ///
    /// As [`StorageAuthority::fact_item`].
    pub fn outbox_item(self, fact: &UsageFact, shard: u8) -> Result<Item, StoreError> {
        self.guard(fact)?;
        let key = self.keys.outbox(&fact.workspace, fact.accepted_sequence)?;
        let attributes = BTreeMap::from([
            (
                "factId".to_owned(),
                ItemValue::text(fact.fact_id.to_string()),
            ),
            (
                "acceptedSequence".to_owned(),
                ItemValue::number(fact.accepted_sequence.get()),
            ),
            (
                "organizationId".to_owned(),
                ItemValue::text(fact.organization.to_string()),
            ),
            (
                "workspaceId".to_owned(),
                ItemValue::text(fact.workspace.to_string()),
            ),
            (
                "region".to_owned(),
                ItemValue::text(fact.region.to_string()),
            ),
            ("category".to_owned(), ItemValue::text(CATEGORY.id())),
            (
                "enqueuedAt".to_owned(),
                ItemValue::instant(fact.accepted_at),
            ),
            ("attempts".to_owned(), ItemValue::number(0u32)),
            // The sparse due-index attributes. Deleting the row on a confirmed
            // send removes it from the index, so the sweep sees only facts that
            // have not been delivered.
            (
                "outDuePk".to_owned(),
                ItemValue::text(format!("OUT#{shard:02}")),
            ),
            (
                "outDueSk".to_owned(),
                ItemValue::text(format!(
                    "{}#{}",
                    fact.accepted_at.to_canonical(),
                    padded(fact.accepted_sequence.get())
                )),
            ),
        ]);
        Ok(Item::new(CATEGORY, key, ItemType::Outbox, attributes)?)
    }

    /// The four-item single-partition admission transaction.
    ///
    /// # Errors
    ///
    /// As [`StorageAuthority::fact_item`].
    pub fn admission(
        self,
        fact: &UsageFact,
        frontier: &Frontier,
        shard: u8,
    ) -> Result<AdmissionTransaction, StoreError> {
        let advanced = Frontier {
            accepted: fact.accepted_sequence,
            ..frontier.clone()
        };
        Ok(AdmissionTransaction {
            // A replayed producer message collides here first, which is what
            // makes the deterministic identity the fence rather than a hint.
            claim: TransactItem {
                item: self.claim_item(fact)?,
                condition: WRITE_ONCE,
            },
            // A fact row is written once and never rewritten.
            fact: TransactItem {
                item: self.fact_item(fact)?,
                condition: WRITE_ONCE,
            },
            // The authority assigns the sequence, so this compare-and-set is
            // what stops a replay forging a frontier position.
            frontier: TransactItem {
                item: self.frontier_item(&advanced)?,
                condition: FRONTIER_CAS,
            },
            outbox: TransactItem {
                item: self.outbox_item(fact, shard)?,
                condition: WRITE_ONCE,
            },
        })
    }

    /// Re-checks a `meter` attribute read back from the table.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::CategoryEscape`] when a sibling authority's meter
    /// appears in this table — an IAM or table defect rather than a producer
    /// one, which is why it is checked separately from the write path.
    pub fn verify_read(self, meter_id: &str) -> Result<(), StoreError> {
        if let Some(meter) = Meter::ALL.into_iter().find(|meter| meter.id() == meter_id) {
            return if meter.category() == CATEGORY {
                Ok(())
            } else {
                Err(StoreError::CategoryEscape {
                    meter: meter.id(),
                    meter_category: meter.category().id(),
                    category: CATEGORY.id(),
                })
            };
        }
        // An observability identifier belongs to the compute authority only.
        if CATEGORY == Category::Compute {
            return Ok(());
        }
        Err(StoreError::CategoryEscape {
            meter: "observability",
            meter_category: Category::Compute.id(),
            category: CATEGORY.id(),
        })
    }
}

/// The attributes of one immutable fact row.
fn fact_attributes(fact: &UsageFact) -> BTreeMap<String, ItemValue> {
    let mut attributes = BTreeMap::from([
        (
            "factId".to_owned(),
            ItemValue::text(fact.fact_id.to_string()),
        ),
        (
            "schemaVersion".to_owned(),
            ItemValue::number(fact.schema_version.get()),
        ),
        ("factKind".to_owned(), ItemValue::text(fact.kind.id())),
        (
            "acceptedSequence".to_owned(),
            ItemValue::number(fact.accepted_sequence.get()),
        ),
        (
            "acceptedAt".to_owned(),
            ItemValue::instant(fact.accepted_at),
        ),
        (
            "organizationId".to_owned(),
            ItemValue::text(fact.organization.to_string()),
        ),
        (
            "workspaceId".to_owned(),
            ItemValue::text(fact.workspace.to_string()),
        ),
        (
            "region".to_owned(),
            ItemValue::text(fact.region.to_string()),
        ),
        (
            "service".to_owned(),
            ItemValue::text(fact.service.to_string()),
        ),
        (
            "resourceKind".to_owned(),
            ItemValue::text(fact.resource.kind.id()),
        ),
        (
            "resourceGeneration".to_owned(),
            ItemValue::text(fact.resource.generation.to_string()),
        ),
        (
            "authorityKind".to_owned(),
            ItemValue::text(fact.authority.kind.id()),
        ),
        (
            "authorityId".to_owned(),
            ItemValue::text(fact.authority.authority_id.as_str().to_owned()),
        ),
        (
            "segmentOrdinal".to_owned(),
            ItemValue::number(fact.authority.segment_ordinal.get()),
        ),
        (
            "pricingVersion".to_owned(),
            ItemValue::text(fact.pricing_version.to_string()),
        ),
        (
            "intentHash".to_owned(),
            ItemValue::text(fact.idempotency.intent_hash.to_string()),
        ),
    ]);

    if let Some(reservation) = &fact.reservation {
        attributes.insert(
            "reservationId".to_owned(),
            ItemValue::text(reservation.to_string()),
        );
    }
    insert_attribution(&mut attributes, fact);
    insert_measurement(&mut attributes, fact);
    insert_correction(&mut attributes, fact);
    attributes
}

/// The optional attribution columns, written only when the work has them.
fn insert_attribution(attributes: &mut BTreeMap<String, ItemValue>, fact: &UsageFact) {
    for (name, value) in [
        (
            "attributionSessionId",
            fact.attribution.session.as_ref().map(ToString::to_string),
        ),
        (
            "attributionAgentId",
            fact.attribution.agent.as_ref().map(ToString::to_string),
        ),
        (
            "attributionRunId",
            fact.attribution.run.as_ref().map(ToString::to_string),
        ),
        (
            "attributionOperationId",
            fact.attribution.operation.as_ref().map(ToString::to_string),
        ),
    ] {
        if let Some(value) = value {
            attributes.insert(name.to_owned(), ItemValue::text(value));
        }
    }
}

/// The measured columns. A void carries none of them: it withdraws its target
/// rather than restating a quantity of its own.
fn insert_measurement(attributes: &mut BTreeMap<String, ItemValue>, fact: &UsageFact) {
    let Some(measurement) = fact.kind.measurement() else {
        return;
    };
    attributes.insert(
        "meter".to_owned(),
        ItemValue::text(measurement.meter().id()),
    );
    attributes.insert(
        "basis".to_owned(),
        ItemValue::text(measurement.basis().id()),
    );
    attributes.insert(
        "quantity".to_owned(),
        ItemValue::N(measurement.quantity().to_string()),
    );
    attributes.insert(
        "serviceTimeStart".to_owned(),
        ItemValue::instant(measurement.service_time().start()),
    );
    attributes.insert(
        "serviceTimeEnd".to_owned(),
        ItemValue::instant(measurement.service_time().end()),
    );
    attributes.insert(
        "sourceReceiptKind".to_owned(),
        ItemValue::text(measurement.source_receipt().kind.id()),
    );
    attributes.insert(
        "sourceReceiptId".to_owned(),
        ItemValue::text(measurement.source_receipt().id.to_string()),
    );
}

/// The correction lineage columns. The target row is never rewritten; the
/// lineage lives on the correction fact instead.
fn insert_correction(attributes: &mut BTreeMap<String, ItemValue>, fact: &UsageFact) {
    let (FactKind::Replace { correction, .. } | FactKind::Void { correction }) = &fact.kind else {
        return;
    };
    attributes.insert(
        "correctionTarget".to_owned(),
        ItemValue::text(correction.target.to_string()),
    );
    attributes.insert(
        "correctionCaseId".to_owned(),
        ItemValue::text(correction.case_id.to_string()),
    );
    attributes.insert(
        "correctionReason".to_owned(),
        ItemValue::text(correction.reason.id()),
    );
}

#[cfg(test)]
mod tests {
    use super::{FRONTIER_CAS, StorageAuthority, StoreError, WRITE_ONCE};
    use aex_usage_domain::correction::{Correction, CorrectionReason};
    use aex_usage_domain::fact::{
        Attribution, FactDraft, FactKind, ResourceGeneration, ResourceKind, SCHEMA_VERSION,
        UsageFact,
    };
    use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
    use aex_usage_domain::identity::{AuthorityId, AuthorityKey, AuthorityKind, SegmentOrdinal};
    use aex_usage_domain::interval::StorageClose;
    use aex_usage_domain::interval::storage::{StorageOwner, StorageOwnerKind, StorageSource};
    use aex_usage_domain::keys::{ItemType, ItemValue};
    use aex_usage_domain::measurement::{
        BoundaryId, Evidence, FactBasis, Measurement, ReceiptKind, ReservationClass, ServiceTime,
        SourceReceipt,
    };
    use aex_usage_domain::meter::{Category, Meter};
    use aex_usage_domain::wire_pending::{
        ActorRef, CaseId, OrganizationId, PricingVersion, RegionId, ServiceId, Timestamp,
        WorkspaceId,
    };

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("representable")
    }

    /// A measurement for the given authority. Each category has one evidence
    /// shape used here, so a test can build "a fact for that table" without
    /// knowing which table it is.
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

    fn fact_in(category: Category, sequence: u64) -> UsageFact {
        fact_with(
            category,
            FactKind::Measured(measurement_for(category)),
            sequence,
        )
    }

    fn fact_with(category: Category, kind: FactKind, sequence: u64) -> UsageFact {
        FactDraft {
            schema_version: SCHEMA_VERSION,
            organization: OrganizationId::parse("org-1").expect("org"),
            workspace: WorkspaceId::parse("ws-1").expect("workspace"),
            region: RegionId::parse("eu-west-1").expect("region"),
            attribution: Attribution::default(),
            service: ServiceId::parse("regional-stream").expect("service"),
            resource: ResourceGeneration {
                kind: ResourceKind::MuxTask,
                generation: Box::from("task-1"),
            },
            authority: AuthorityKey {
                region: RegionId::parse("eu-west-1").expect("region"),
                category,
                kind: authority_kind(category),
                authority_id: AuthorityId::parse("a-1").expect("id"),
                segment_ordinal: SegmentOrdinal::FIRST,
            },
            pricing_version: PricingVersion::parse("synthetic-zero-v1").expect("version"),
            reservation: None,
            kind,
        }
        .admit(
            AcceptedSequence::new(sequence).expect("positive"),
            at(600_000),
        )
        .expect("admits")
    }

    fn frontier_in(category: Category) -> Frontier {
        Frontier::empty(
            RegionId::parse("eu-west-1").expect("region"),
            WorkspaceId::parse("ws-1").expect("workspace"),
            category,
        )
    }

    /// The two authorities this crate must never be able to write.
    fn siblings() -> Vec<Category> {
        Category::ALL
            .into_iter()
            .filter(|candidate| *candidate != crate::CATEGORY)
            .collect()
    }

    #[test]
    fn the_adapter_addresses_exactly_one_authority() {
        assert_eq!(StorageAuthority::new().category(), crate::CATEGORY);
        assert_eq!(siblings().len(), 2, "there are three authorities");
    }

    #[test]
    fn a_native_fact_row_carries_its_declared_attribute_set() {
        let fact = fact_in(crate::CATEGORY, 7);
        let item = StorageAuthority::new().fact_item(&fact).expect("builds");

        assert_eq!(item.item_type, ItemType::Fact);
        assert_eq!(item.key.pk, "WS#ws-1");
        assert_eq!(item.key.sk, "FACT#00000000000000000007");

        let map = item.to_attribute_map();
        assert_eq!(map["itemType"], ItemValue::text("usage_fact"));
        assert_eq!(map["acceptedSequence"], ItemValue::N("7".to_owned()));
        assert_eq!(map["pricingVersion"], ItemValue::text("synthetic-zero-v1"));

        let measurement = fact.kind.measurement().expect("measured");
        assert_eq!(map["meter"], ItemValue::text(measurement.meter().id()));
        assert_eq!(
            map["quantity"],
            ItemValue::N(measurement.quantity().to_string())
        );

        // Every money-relevant attribute is written rather than optional.
        for required in [
            "factId",
            "schemaVersion",
            "factKind",
            "acceptedAt",
            "organizationId",
            "workspaceId",
            "region",
            "service",
            "resourceKind",
            "resourceGeneration",
            "authorityKind",
            "authorityId",
            "segmentOrdinal",
            "intentHash",
            "basis",
            "serviceTimeStart",
            "serviceTimeEnd",
            "sourceReceiptKind",
            "sourceReceiptId",
        ] {
            assert!(map.contains_key(required), "`{required}` must be written");
        }
    }

    #[test]
    fn a_sibling_authority_fact_is_refused_on_every_write_path() {
        let authority = StorageAuthority::new();
        for sibling in siblings() {
            let foreign = fact_in(sibling, 1);
            for outcome in [
                authority.fact_item(&foreign).err(),
                authority.claim_item(&foreign).err(),
                authority.outbox_item(&foreign, 0).err(),
                authority
                    .admission(&foreign, &frontier_in(sibling), 0)
                    .err(),
            ] {
                assert!(
                    matches!(outcome, Some(StoreError::CategoryEscape { .. })),
                    "a {sibling} fact must never reach the {} table",
                    crate::CATEGORY
                );
            }
        }
    }

    #[test]
    fn a_sibling_authority_meter_is_refused_again_on_read() {
        let authority = StorageAuthority::new();
        for meter in Meter::ALL {
            let outcome = authority.verify_read(meter.id());
            if meter.category() == crate::CATEGORY {
                assert!(outcome.is_ok(), "{meter} belongs here");
            } else {
                assert!(
                    matches!(outcome, Err(StoreError::CategoryEscape { .. })),
                    "{meter} must be refused on read as well as on write"
                );
            }
        }
    }

    #[test]
    fn a_sibling_authority_frontier_is_refused() {
        let authority = StorageAuthority::new();
        for sibling in siblings() {
            assert!(matches!(
                authority.frontier_item(&frontier_in(sibling)),
                Err(StoreError::CategoryEscape { .. })
            ));
        }
        assert!(
            authority
                .frontier_item(&frontier_in(crate::CATEGORY))
                .is_ok()
        );
    }

    #[test]
    fn the_admission_transaction_is_four_items_in_one_partition() {
        let fact = fact_in(crate::CATEGORY, 1);
        let transaction = StorageAuthority::new()
            .admission(&fact, &frontier_in(crate::CATEGORY), 3)
            .expect("builds");

        let items = transaction.items();
        assert_eq!(items.len(), 4);
        for item in items {
            assert_eq!(
                item.item.key.pk, "WS#ws-1",
                "a single-partition transaction keeps the frontier CAS local"
            );
        }

        assert_eq!(transaction.claim.condition, WRITE_ONCE);
        assert_eq!(transaction.fact.condition, WRITE_ONCE);
        assert_eq!(transaction.outbox.condition, WRITE_ONCE);
        assert_eq!(transaction.frontier.condition, FRONTIER_CAS);

        assert_eq!(transaction.claim.item.item_type, ItemType::FactClaim);
        assert_eq!(transaction.fact.item.item_type, ItemType::Fact);
        assert_eq!(transaction.frontier.item.item_type, ItemType::Frontier);
        assert_eq!(transaction.outbox.item.item_type, ItemType::Outbox);

        // The transaction advances the frontier to exactly this fact.
        let map = transaction.frontier.item.to_attribute_map();
        assert_eq!(map["acceptedSequence"], ItemValue::N("1".to_owned()));
    }

    #[test]
    fn the_claim_carries_the_intent_hash_the_replay_check_compares() {
        let fact = fact_in(crate::CATEGORY, 4);
        let claim = StorageAuthority::new().claim_item(&fact).expect("builds");
        let map = claim.to_attribute_map();

        assert_eq!(claim.key.sk, format!("ID#{}", fact.fact_id));
        assert_eq!(
            map["intentHash"],
            ItemValue::text(fact.idempotency.intent_hash.to_string()),
            "a retry with a different quantity must be distinguishable from a replay"
        );
        assert_eq!(map["acceptedSequence"], ItemValue::N("4".to_owned()));
    }

    #[test]
    fn the_outbox_row_carries_the_sparse_due_index_attributes() {
        let fact = fact_in(crate::CATEGORY, 9);
        let item = StorageAuthority::new()
            .outbox_item(&fact, 5)
            .expect("builds");
        let map = item.to_attribute_map();

        assert_eq!(map["outDuePk"], ItemValue::text("OUT#05"));
        assert_eq!(
            map["outDueSk"],
            ItemValue::text(format!(
                "{}#00000000000000000009",
                fact.accepted_at.to_canonical()
            )),
            "the due index orders by enqueue instant then sequence"
        );
        assert_eq!(map["attempts"], ItemValue::N("0".to_owned()));
        assert_eq!(map["category"], ItemValue::text(crate::CATEGORY.id()));
    }

    #[test]
    fn a_quarantined_frontier_records_where_and_why_it_parked() {
        let parked = frontier_in(crate::CATEGORY)
            .admit(AcceptedSequence::new(1).expect("one"))
            .expect("admits")
            .quarantine(
                AcceptedSequence::new(1).expect("one"),
                PoisonReason::Undecodable,
            );
        let map = StorageAuthority::new()
            .frontier_item(&parked)
            .expect("builds")
            .to_attribute_map();

        assert_eq!(map["state"], ItemValue::text("quarantined"));
        assert_eq!(map["quarantineAt"], ItemValue::N("1".to_owned()));
        assert_eq!(map["quarantineReason"], ItemValue::text("undecodable"));
        assert_eq!(map["acceptedSequence"], ItemValue::N("1".to_owned()));
    }

    #[test]
    fn a_void_records_its_lineage_and_carries_no_quantity_of_its_own() {
        let target = fact_in(crate::CATEGORY, 1);
        let void = fact_with(
            crate::CATEGORY,
            FactKind::Void {
                correction: Correction {
                    case_id: CaseId::parse("case-1").expect("case"),
                    target: target.fact_id.clone(),
                    prior_head: None,
                    reason: CorrectionReason::DuplicateMeasurement,
                    actor: ActorRef::Reconciler {
                        service: ServiceId::parse("provider-cost-reconciler").expect("service"),
                    },
                },
            },
            2,
        );

        let map = StorageAuthority::new()
            .fact_item(&void)
            .expect("builds")
            .to_attribute_map();
        assert_eq!(map["factKind"], ItemValue::text("void"));
        assert_eq!(
            map["correctionTarget"],
            ItemValue::text(target.fact_id.to_string())
        );
        assert_eq!(
            map["correctionReason"],
            ItemValue::text("duplicate_measurement")
        );
        assert!(
            !map.contains_key("quantity"),
            "a void carries no quantity; the projection subtracts its target's"
        );
    }

    #[test]
    fn the_same_fact_always_produces_the_same_row() {
        // Determinism at the codec, not only at the identity: a replayed
        // producer message must build an identical row, or the
        // `attribute_not_exists` fence would be masking a real difference.
        let first = fact_in(crate::CATEGORY, 3);
        let second = fact_in(crate::CATEGORY, 3);
        let authority = StorageAuthority::new();
        assert_eq!(
            authority.fact_item(&first).expect("builds"),
            authority.fact_item(&second).expect("builds")
        );
        assert_eq!(first.fact_id, second.fact_id);
    }
}
