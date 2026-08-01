//! The `AuthorityStore` implementation over `DynamoDB`.
//!
//! One store, one table, one category. The table name is supplied at
//! construction and the category is [`crate::CATEGORY`], so there is no code
//! path here that could address a sibling authority even with the wrong
//! credentials.
//!
//! # The two conditions that decide correctness
//!
//! - **Admission** is the four-item single-partition transaction. The identity
//!   claim carries `attribute_not_exists`, so a producer retry collides there
//!   first and is resolved by re-reading rather than by a second write. The
//!   frontier carries a compare-and-set on its previous position, so two
//!   concurrent producers cannot both take one sequence.
//! - **Every stage advance** is a compare-and-set over the whole four-stage
//!   position, not over the one stage being moved. A caller that read a frontier,
//!   decided, and then found any stage moved underneath must re-read and
//!   re-decide; silently winning would let two workers disagree about what has
//!   been folded.

use std::collections::HashMap;
use std::sync::Arc;

use aex_usage_application::ports::{
    Admission, AuthorityStore, Clock, FactLocation, OutboxEntry, PortError, ReceiptOutcome,
    SettlementReceipt,
};
use aex_usage_domain::fact::{FactDraft, UsageFact};
use aex_usage_domain::frontier::{AcceptedSequence, Frontier, PoisonReason};
use aex_usage_domain::identity::FactId;
use aex_usage_domain::keys::{AuthorityKeys, ItemType, padded};
use aex_usage_domain::meter::Category;
use aex_usage_domain::wire_pending::{RegionId, Timestamp, WorkspaceId};
use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{AttributeValue, Put, TransactWriteItem};
use xxhash_rust::xxh3::xxh3_64;

use crate::attribute;
use crate::codec::{self, DecodeError};
use crate::expressions::{ReceiptRow, StoreError, TransactItem, TransferAuthority, WRITE_ONCE};
use crate::fault::{Idempotence, classify};
use crate::{CATEGORY, gsi};

/// How many shards the outbox due index is spread over.
///
/// Re-exported from the application rather than restated, so the sweep and the
/// writer cannot disagree about how many pages one sweep has to read.
pub use aex_usage_application::worker::OUTBOX_SHARDS;

/// The name this store reports in a port error.
const WHAT: &str = "usage authority";

/// The `usage-transfer-authority` port implementation.
#[derive(Debug, Clone)]
pub struct TransferStore {
    client: Client,
    table: String,
    region: RegionId,
    clock: Arc<dyn Clock>,
    expressions: TransferAuthority,
    keys: AuthorityKeys,
}

impl TransferStore {
    /// Binds a store to a client, a physical table name and its region.
    ///
    /// The region is held rather than derived: an absent frontier row is the
    /// empty frontier of *this* region, and inferring that from a client's
    /// configuration would make a mis-set client silently mint a foreign
    /// sequence.
    #[must_use]
    pub fn new(
        client: Client,
        table: impl Into<String>,
        region: RegionId,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            client,
            table: table.into(),
            region,
            clock,
            expressions: TransferAuthority::new(),
            keys: AuthorityKeys::new(CATEGORY),
        }
    }

    /// The physical table name.
    #[must_use]
    pub fn table(&self) -> &str {
        &self.table
    }

    /// The region this store's sequences belong to.
    #[must_use]
    pub const fn region(&self) -> &RegionId {
        &self.region
    }

    /// Reads one row by key.
    async fn get(
        &self,
        key: HashMap<String, AttributeValue>,
    ) -> Result<Option<HashMap<String, AttributeValue>>, PortError> {
        let output = self
            .client
            .get_item()
            .table_name(&self.table)
            .set_key(Some(key))
            // Money evidence is never read eventually: a stale frontier would
            // mint a sequence that is already taken.
            .consistent_read(true)
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Read, &error))?;
        Ok(output.item)
    }

    /// Renders one conditional write, binding any extra condition values.
    fn put(
        &self,
        write: &TransactItem,
        values: Vec<(&'static str, AttributeValue)>,
    ) -> Result<TransactWriteItem, PortError> {
        let mut builder = Put::builder()
            .table_name(&self.table)
            .set_item(Some(attribute::item(&write.item)))
            .condition_expression(write.condition);
        for (name, value) in values {
            builder = builder.expression_attribute_values(name, value);
        }
        let put = builder.build().map_err(|error| PortError::Corrupt {
            what: "authority write",
            reason: error.to_string(),
        })?;
        Ok(TransactWriteItem::builder().put(put).build())
    }
}

#[async_trait]
impl AuthorityStore for TransferStore {
    fn category(&self) -> Category {
        CATEGORY
    }

    async fn admit(&self, draft: &FactDraft, at: Timestamp) -> Result<Admission, PortError> {
        let frontier = self.frontier(&draft.workspace).await?;
        let sequence = frontier
            .accepted
            .next()
            .map_err(|error| PortError::Corrupt {
                what: "accepted sequence",
                reason: error.to_string(),
            })?;
        let fact = draft
            .clone()
            .admit(sequence, at)
            .map_err(|error| PortError::Corrupt {
                what: "fact draft",
                reason: error.to_string(),
            })?;
        let transaction = self
            .expressions
            .admission(&fact, &frontier, shard_of(&fact.workspace))
            .map_err(|error| store_error(&error))?;

        let items = vec![
            self.put(&transaction.claim, Vec::new())?,
            self.put(&transaction.fact, Vec::new())?,
            self.put(
                &transaction.frontier,
                vec![(
                    ":previousAcceptedSequence",
                    attribute::number(frontier.accepted.get()),
                )],
            )?,
            self.put(&transaction.outbox, Vec::new())?,
        ];
        debug_assert_eq!(items.len(), transaction.items().len());

        let outcome = self
            .client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(Admission::Admitted(Box::new(fact))),
            Err(error) => {
                let claim_lost = cancelled_on_claim(&error);
                if claim_lost {
                    // The deterministic identity is the fence, so a collision
                    // here is resolved by reading what is already stored rather
                    // than by writing again.
                    return self.resolve_claim(draft).await;
                }
                Err(classify(WHAT, Idempotence::Write, &error))
            }
        }
    }

    async fn frontier(&self, workspace: &WorkspaceId) -> Result<Frontier, PortError> {
        let key = self
            .keys
            .frontier(workspace)
            .map_err(|error| key_error(&error))?;
        match self.get(attribute::key(&key)).await? {
            // An absent row is the empty frontier of this region, which is what
            // a workspace that has never been billed actually has.
            None => Ok(Frontier::empty(
                self.region.clone(),
                workspace.clone(),
                CATEGORY,
            )),
            Some(item) => codec::decode_frontier(&item).map_err(|error| decode_error(&error)),
        }
    }

    async fn advance_frontier(&self, from: &Frontier, to: &Frontier) -> Result<(), PortError> {
        if from.workspace != to.workspace || from.category != to.category {
            return Err(PortError::Corrupt {
                what: "frontier advance",
                reason: "the two positions describe different sequences".to_owned(),
            });
        }
        let item = self
            .expressions
            .frontier_item(to)
            .map_err(|error| store_error(&error))?;
        self.client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(attribute::item(&item)))
            // The compare-and-set covers all four stages, not just the one being
            // moved: a caller that read, decided and lost any stage must re-read
            // and re-decide rather than silently winning.
            .condition_expression(
                "attribute_not_exists(pk) OR (acceptedSequence = :fromAccepted \
                 AND projectedSequence = :fromProjected \
                 AND publishedSequence = :fromPublished \
                 AND settledSequence = :fromSettled)",
            )
            .expression_attribute_values(":fromAccepted", attribute::number(from.accepted.get()))
            .expression_attribute_values(":fromProjected", attribute::number(from.projected.get()))
            .expression_attribute_values(":fromPublished", attribute::number(from.published.get()))
            .expression_attribute_values(":fromSettled", attribute::number(from.settled.get()))
            .send()
            .await
            .map_err(|error| {
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
                    )
                }) {
                    return PortError::Conflict { what: "frontier" };
                }
                classify(WHAT, Idempotence::Write, &error)
            })?;
        Ok(())
    }

    async fn fact_at(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<Option<UsageFact>, PortError> {
        let key = self
            .keys
            .fact(workspace, sequence)
            .map_err(|error| key_error(&error))?;
        match self.get(attribute::key(&key)).await? {
            None => Ok(None),
            Some(item) => codec::decode_fact(&item)
                .map(Some)
                .map_err(|error| decode_error(&error)),
        }
    }

    async fn locate(&self, fact: &FactId) -> Result<Option<FactLocation>, PortError> {
        // `gsi_fact_id` is sparse on `factId`, and a fact row, an outbox row and
        // a receipt row all carry that attribute as well as the claim. The
        // filter is therefore on the discriminator, which both indexes project
        // explicitly for exactly this reason.
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(gsi::FACT_ID)
            .key_condition_expression("#factId = :factId")
            .filter_expression("#itemType = :claim")
            .expression_attribute_names("#factId", "factId")
            .expression_attribute_names("#itemType", attribute::ITEM_TYPE)
            .expression_attribute_values(":factId", attribute::text(fact.as_str()))
            .expression_attribute_values(":claim", attribute::text(ItemType::FactClaim.id()))
            .limit(2)
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Read, &error))?;

        let mut located = None;
        for item in output.items.unwrap_or_default() {
            let claim = codec::decode_claim(&item).map_err(|error| decode_error(&error))?;
            if located.is_some() {
                // One deterministic identity lives in one partition. Two claims
                // means two workspaces minted one fact id, which is a defect the
                // caller must not resolve by picking one.
                return Err(PortError::Corrupt {
                    what: "usage_fact_claim",
                    reason: format!("`{fact}` is claimed in more than one partition"),
                });
            }
            located = Some(FactLocation {
                workspace: claim.workspace,
                sequence: claim.accepted_sequence,
            });
        }
        Ok(located)
    }

    async fn quarantine(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        reason: PoisonReason,
        detail: &str,
    ) -> Result<(), PortError> {
        let parked_at = self.clock.now();
        let marker = self
            .expressions
            .quarantine_item(workspace, sequence, reason, detail, parked_at)
            .map_err(|error| store_error(&error))?;
        let frontier_key = self
            .keys
            .frontier(workspace)
            .map_err(|error| key_error(&error))?;

        // Two writes in one commit: the marker records what could not be folded,
        // and the frontier parks. An update rather than a put on the frontier,
        // because a workspace whose very first record is poison has no frontier
        // row yet and must still park rather than carry on with none.
        let items = vec![
            TransactWriteItem::builder()
                .put(
                    Put::builder()
                        .table_name(&self.table)
                        .set_item(Some(attribute::item(&marker)))
                        .build()
                        .map_err(|error| PortError::Corrupt {
                            what: "quarantine marker",
                            reason: error.to_string(),
                        })?,
                )
                .build(),
            TransactWriteItem::builder()
                .update(
                    aws_sdk_dynamodb::types::Update::builder()
                        .table_name(&self.table)
                        .set_key(Some(attribute::key(&frontier_key)))
                        .update_expression(
                            "SET #itemType = :itemType, #category = :category, \
                             #region = :region, #workspace = :workspace, \
                             #accepted = if_not_exists(#accepted, :origin), \
                             #projected = if_not_exists(#projected, :origin), \
                             #published = if_not_exists(#published, :origin), \
                             #settled = if_not_exists(#settled, :origin), \
                             #state = :quarantined, #at = :at, #reason = :reason",
                        )
                        .expression_attribute_names("#itemType", attribute::ITEM_TYPE)
                        .expression_attribute_names("#category", "category")
                        .expression_attribute_names("#region", "region")
                        .expression_attribute_names("#workspace", "workspaceId")
                        .expression_attribute_names("#accepted", "acceptedSequence")
                        .expression_attribute_names("#projected", "projectedSequence")
                        .expression_attribute_names("#published", "publishedSequence")
                        .expression_attribute_names("#settled", "settledSequence")
                        .expression_attribute_names("#state", "state")
                        .expression_attribute_names("#at", "quarantineAt")
                        .expression_attribute_names("#reason", "quarantineReason")
                        .expression_attribute_values(
                            ":itemType",
                            attribute::text(ItemType::Frontier.id()),
                        )
                        .expression_attribute_values(":category", attribute::text(CATEGORY.id()))
                        .expression_attribute_values(
                            ":region",
                            attribute::text(self.region.as_str()),
                        )
                        .expression_attribute_values(
                            ":workspace",
                            attribute::text(workspace.as_str()),
                        )
                        .expression_attribute_values(":origin", attribute::number(0u32))
                        .expression_attribute_values(":quarantined", attribute::text("quarantined"))
                        .expression_attribute_values(":at", attribute::number(sequence.get()))
                        .expression_attribute_values(":reason", attribute::text(reason.id()))
                        .build()
                        .map_err(|error| PortError::Corrupt {
                            what: "frontier park",
                            reason: error.to_string(),
                        })?,
                )
                .build(),
        ];
        self.client
            .transact_write_items()
            .set_transact_items(Some(items))
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Write, &error))?;
        Ok(())
    }

    async fn record_receipt(
        &self,
        receipt: &SettlementReceipt,
        location: &FactLocation,
    ) -> Result<ReceiptOutcome, PortError> {
        let item = self
            .expressions
            .receipt_item(
                &ReceiptRow {
                    receipt_id: &receipt.receipt_id,
                    region: &receipt.region,
                    category: receipt.category,
                    fact_id: &receipt.fact_id,
                    rated_microusd: receipt.rated_microusd,
                    transaction_id: &receipt.transaction_id,
                    pricing_version: &receipt.pricing_version,
                    settled_at: receipt.settled_at,
                },
                &location.workspace,
                location.sequence,
            )
            .map_err(|error| store_error(&error))?;
        let outcome = self
            .client
            .put_item()
            .table_name(&self.table)
            .set_item(Some(attribute::item(&item)))
            .condition_expression(WRITE_ONCE)
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(ReceiptOutcome::Recorded),
            Err(error)
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_)
                    )
                }) =>
            {
                // Write-once, so a redelivered receipt is an expected condition
                // rather than a fault: central's outbox replays.
                Ok(ReceiptOutcome::AlreadyRecorded)
            }
            Err(error) => Err(classify(WHAT, Idempotence::Write, &error)),
        }
    }

    async fn has_receipt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<bool, PortError> {
        let key = self
            .keys
            .receipt(workspace, sequence)
            .map_err(|error| key_error(&error))?;
        Ok(self.get(attribute::key(&key)).await?.is_some())
    }

    async fn discard_outbox(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
    ) -> Result<(), PortError> {
        let key = self
            .keys
            .outbox(workspace, sequence)
            .map_err(|error| key_error(&error))?;
        self.client
            .delete_item()
            .table_name(&self.table)
            .set_key(Some(attribute::key(&key)))
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Write, &error))?;
        Ok(())
    }

    async fn due_outbox(
        &self,
        shard: u8,
        older_than: Timestamp,
        limit: usize,
    ) -> Result<Vec<OutboxEntry>, PortError> {
        let output = self
            .client
            .query()
            .table_name(&self.table)
            .index_name(gsi::OUTBOX_DUE)
            .key_condition_expression("#pk = :shard AND #sk <= :bound")
            .expression_attribute_names("#pk", gsi::OUTBOX_DUE_PARTITION)
            .expression_attribute_names("#sk", gsi::OUTBOX_DUE_SORT)
            .expression_attribute_values(":shard", attribute::text(due_partition(shard)))
            // The bound is the largest sort key that instant can carry, so a row
            // enqueued exactly at the boundary is included rather than skipped
            // forever by a bound that stops one byte short.
            .expression_attribute_values(
                ":bound",
                attribute::text(format!(
                    "{}#{}",
                    older_than.to_canonical(),
                    padded(u64::MAX)
                )),
            )
            .limit(i32::try_from(limit).unwrap_or(i32::MAX))
            .send()
            .await
            .map_err(|error| classify(WHAT, Idempotence::Read, &error))?;

        let mut entries = Vec::new();
        for item in output.items.unwrap_or_default() {
            let row = codec::decode_outbox(&item).map_err(|error| decode_error(&error))?;
            entries.push(OutboxEntry {
                workspace: row.workspace,
                organization: row.organization,
                region: row.region,
                category: CATEGORY,
                fact_id: row.fact_id,
                accepted_sequence: row.accepted_sequence,
                enqueued_at: row.enqueued_at,
                attempts: row.attempts,
            });
        }
        Ok(entries)
    }

    async fn note_outbox_attempt(
        &self,
        workspace: &WorkspaceId,
        sequence: AcceptedSequence,
        attempts: u32,
        at: Timestamp,
    ) -> Result<(), PortError> {
        let key = self
            .keys
            .outbox(workspace, sequence)
            .map_err(|error| key_error(&error))?;
        let outcome = self
            .client
            .update_item()
            .table_name(&self.table)
            .set_key(Some(attribute::key(&key)))
            // Re-stamping the due sort key is what moves a swept row to the back
            // of its shard, so one stuck row cannot starve the rest of the page.
            .update_expression("SET #attempts = :attempts, #enqueued = :at, #dueSk = :dueSk")
            .condition_expression("attribute_exists(pk)")
            .expression_attribute_names("#attempts", "attempts")
            .expression_attribute_names("#enqueued", "enqueuedAt")
            .expression_attribute_names("#dueSk", gsi::OUTBOX_DUE_SORT)
            .expression_attribute_values(":attempts", attribute::number(attempts))
            .expression_attribute_values(":at", attribute::text(at.to_canonical()))
            .expression_attribute_values(
                ":dueSk",
                attribute::text(format!("{}#{}", at.to_canonical(), padded(sequence.get()))),
            )
            .send()
            .await;
        match outcome {
            Ok(_) => Ok(()),
            Err(error)
                if error.as_service_error().is_some_and(|service| {
                    matches!(
                        service,
                        aws_sdk_dynamodb::operation::update_item::UpdateItemError::ConditionalCheckFailedException(_)
                    )
                }) =>
            {
                // The row was delivered and discarded between the page read and
                // this note. Recreating it would resurrect a settled fact into
                // the outbox, so the absence is the answer.
                Ok(())
            }
            Err(error) => Err(classify(WHAT, Idempotence::Write, &error)),
        }
    }
}

impl TransferStore {
    /// Resolves an admission that lost the identity claim.
    async fn resolve_claim(&self, draft: &FactDraft) -> Result<Admission, PortError> {
        let fact_id = draft.fact_id();
        let offered = draft.intent_hash().map_err(|error| PortError::Corrupt {
            what: "fact draft",
            reason: error.to_string(),
        })?;
        let key = self
            .keys
            .claim(&draft.workspace, &fact_id)
            .map_err(|error| key_error(&error))?;
        let claim = self
            .get(attribute::key(&key))
            .await?
            .ok_or(PortError::Conflict { what: "admission" })?;
        let claim = codec::decode_claim(&claim).map_err(|error| decode_error(&error))?;
        let stored = self
            .fact_at(&claim.workspace, claim.accepted_sequence)
            .await?
            .ok_or_else(|| PortError::NotFound {
                what: "usage_fact",
                id: fact_id.to_string(),
            })?;
        if stored.idempotency.intent_hash == offered {
            return Ok(Admission::Replayed(Box::new(stored)));
        }
        // Two different quantities cannot both be true of one physical
        // measurement, so nothing is written and the producer is told.
        Ok(Admission::IdentityConflict {
            fact_id,
            stored: stored.idempotency.intent_hash,
            offered,
        })
    }
}

/// Whether an admission was cancelled because the identity claim already exists.
fn cancelled_on_claim<R>(
    error: &aws_sdk_dynamodb::error::SdkError<
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        R,
    >,
) -> bool {
    let Some(
        aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::TransactionCanceledException(
            cancelled,
        ),
    ) = error.as_service_error()
    else {
        return false;
    };
    // Item 0 is the `ID#` claim; see `AdmissionTransaction::items`.
    cancelled
        .cancellation_reasons()
        .first()
        .and_then(|reason| reason.code())
        == Some("ConditionalCheckFailed")
}

/// The due-index partition one shard occupies.
#[must_use]
pub fn due_partition(shard: u8) -> String {
    format!("OUT#{shard:02}")
}

/// Which sweep shard one workspace's outbox rows land on.
///
/// Shard selection only. The shard decides which sweep page a row appears on,
/// never where it is stored, so this never needs to be a cryptographic digest.
#[must_use]
pub fn shard_of(workspace: &WorkspaceId) -> u8 {
    let shards = u64::from(OUTBOX_SHARDS);
    u8::try_from(xxh3_64(workspace.as_str().as_bytes()) % shards).unwrap_or(0)
}

/// A key-grammar failure is never retryable: the same key never builds.
fn key_error(error: &aex_usage_domain::keys::KeyError) -> PortError {
    PortError::Corrupt {
        what: "authority key",
        reason: error.to_string(),
    }
}

/// A refused write is a producer or wiring defect, never a transient one.
fn store_error(error: &StoreError) -> PortError {
    PortError::Corrupt {
        what: "authority row",
        reason: error.to_string(),
    }
}

/// A row that does not decode never decodes.
fn decode_error(error: &DecodeError) -> PortError {
    PortError::Corrupt {
        what: "authority row",
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{OUTBOX_SHARDS, due_partition, shard_of};
    use aex_usage_domain::wire_pending::WorkspaceId;
    use std::collections::BTreeSet;

    fn workspace(value: &str) -> WorkspaceId {
        WorkspaceId::parse(value).expect("workspace")
    }

    #[test]
    fn a_workspace_always_lands_on_the_same_shard() {
        // The shard decides which sweep page a row appears on. If it moved, a
        // row enqueued under one shard would never be read by any sweep.
        let first = shard_of(&workspace("ws-1"));
        for _ in 0..8 {
            assert_eq!(shard_of(&workspace("ws-1")), first);
        }
        assert!(first < OUTBOX_SHARDS);
    }

    #[test]
    fn every_shard_is_reachable_so_no_sweep_page_is_dead() {
        let occupied: BTreeSet<u8> = (0..2_000)
            .map(|index| shard_of(&workspace(&format!("ws-{index}"))))
            .collect();
        assert_eq!(
            occupied.len(),
            usize::from(OUTBOX_SHARDS),
            "a shard no workspace lands on is a page the sweep reads for nothing"
        );
    }

    #[test]
    fn the_due_partition_is_fixed_width_so_it_cannot_collide() {
        assert_eq!(due_partition(0), "OUT#00");
        assert_eq!(due_partition(15), "OUT#15");
        let widths: BTreeSet<usize> = (0..OUTBOX_SHARDS)
            .map(|shard| due_partition(shard).len())
            .collect();
        assert_eq!(widths.len(), 1);
    }
}
