//! One SQS invocation, from the delivered batch to the partial-batch response.

use std::collections::BTreeSet;
use std::sync::Arc;

use aex_finance_app::use_cases::RatingMessage;
use aex_wire::ids::OrganizationId;
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEvent};

use super::backlog::{Delivered, MESSAGE_GROUP_ATTRIBUTE, chunks, partition, uncommitted};
use super::settle::{SettleError, SettlementAuthority};

/// What one invocation observed, for the record it emits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// How many accounts the batch touched.
    pub accounts: usize,
    /// How many facts became durable for the first time.
    pub claimed: usize,
    /// How many facts a duplicate delivery had already stored.
    pub duplicates: usize,
    /// How many durable facts still await rating and posting.
    pub pending: usize,
    /// How many facts were quarantined on an intent conflict.
    pub quarantined: usize,
    /// How many accounts did not reach a complete settlement receipt.
    pub failed_accounts: usize,
    /// How many delivered messages could not be decoded or grouped.
    pub undecodable: usize,
}

/// Decodes one delivered SQS batch, one record at a time.
///
/// The typed event layer once decoded the whole batch at the runtime boundary,
/// so one undecodable body failed every message — including other accounts' —
/// into the DLQ. Each body is decoded here instead, and only what actually
/// failed is returned to the queue.
///
/// Two refusals are deliberate:
///
/// - a message with no identity or no FIFO group is not silently dropped: it
///   is returned as a failure so it goes back to the queue and is visible;
/// - an undecodable body fails **its own group's tail** as well. The queue is
///   FIFO per account; settling an account's later facts ahead of an earlier
///   one it cannot decode would reorder the ledger the grouping exists to
///   protect. Other groups proceed untouched.
#[must_use]
pub fn decode(event: SqsEvent) -> (Vec<Delivered>, Vec<String>) {
    let mut delivered = Vec::with_capacity(event.records.len());
    let mut failed = Vec::new();
    let mut poisoned_groups: BTreeSet<String> = BTreeSet::new();
    for record in event.records {
        let Some(message_id) = record.message_id.clone() else {
            // With no identity there is nothing a partial-batch response could
            // name; the runtime acknowledges it either way.
            continue;
        };
        let group = record
            .attributes
            .get(MESSAGE_GROUP_ATTRIBUTE)
            .cloned()
            .unwrap_or_default();
        if group.is_empty() {
            failed.push(message_id);
            continue;
        }
        if poisoned_groups.contains(&group) {
            failed.push(message_id);
            continue;
        }
        if let Ok(request) =
            serde_json::from_str::<RatingMessage>(record.body.as_deref().unwrap_or_default())
        {
            delivered.push(Delivered {
                message_id,
                group,
                request,
            });
        } else {
            poisoned_groups.insert(group);
            failed.push(message_id);
        }
    }
    (delivered, failed)
}

/// Settles one delivered batch and reports which messages did not commit.
///
/// # Errors
///
/// Never returns an error: a settlement failure is expressed as a partial-batch
/// item so the rest of the batch is still acknowledged. Returning an error would
/// make SQS redeliver work that is already durable.
pub async fn handle<A: SettlementAuthority>(
    authority: &Arc<A>,
    event: SqsEvent,
    max_group_batch: u32,
    serialization_retry_max: u32,
) -> (SqsBatchResponse, BatchReport) {
    let mut response = SqsBatchResponse::default();
    let mut report = BatchReport::default();

    let (delivered, undecodable) = decode(event);
    report.undecodable = undecodable.len();
    for message_id in undecodable {
        response.add_failure(message_id);
    }

    // A batch whose grouping cannot be trusted is returned whole: the ordering
    // guarantee is the reason this queue is FIFO at all. "Whole" means every
    // delivered message is named — a message the partial-batch response does
    // not name is acknowledged and deleted, which would silently drop money.
    let delivered_ids: Vec<String> = delivered
        .iter()
        .map(|message| message.message_id.clone())
        .collect();
    let Ok(groups) = partition(delivered) else {
        report.undecodable += delivered_ids.len();
        for message_id in delivered_ids {
            response.add_failure(message_id);
        }
        return (response, report);
    };
    report.accounts = groups.len();

    let mut failed: Vec<OrganizationId> = Vec::new();
    for group in &groups {
        let mut settled = true;
        for chunk in chunks(group, max_group_batch) {
            let result = settle_with_retry(
                authority,
                group.organization,
                &chunk,
                serialization_retry_max,
            )
            .await;
            let Ok(outcome) = result else {
                settled = false;
                break;
            };
            report.claimed += outcome.claimed;
            report.duplicates += outcome.duplicates;
            report.pending += outcome.pending;
            report.quarantined += outcome.quarantined.len();
            if outcome.pending > 0 || !outcome.quarantined.is_empty() {
                // The transaction committed an inbox state, but no settlement
                // receipt exists. SQS acknowledgement is about completed work,
                // not merely about whether one intermediate write committed.
                settled = false;
                break;
            }
        }
        if !settled {
            failed.push(group.organization);
        }
    }
    report.failed_accounts = failed.len();
    for message_id in uncommitted(&groups, &failed) {
        response.add_failure(message_id);
    }
    (response, report)
}

/// Settles one chunk, retrying only a serialization failure.
async fn settle_with_retry<A: SettlementAuthority>(
    authority: &Arc<A>,
    organization: OrganizationId,
    chunk: &[Delivered],
    retry_max: u32,
) -> Result<super::settle::GroupOutcome, SettleError> {
    let mut attempt = 0;
    loop {
        match authority.settle(organization, chunk).await {
            Ok(outcome) => return Ok(outcome),
            Err(error) if error.is_retryable() && attempt < retry_max => {
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aex_finance_app::use_cases::RatingRequest;
    use aex_internal_contracts::usage::{
        Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
        ServiceTime, SourceReceipt, UsageFact,
    };
    use aex_internal_contracts::{PricingVersion, SchemaVersion};
    use aex_wire::PrefixedId as _;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{OrganizationId, WorkspaceId};
    use aex_wire::types::{DecimalU128, Region, Timestamp};
    use aws_lambda_events::sqs::{SqsEvent, SqsMessage};

    use super::super::backlog::{Delivered, MESSAGE_GROUP_ATTRIBUTE};
    use super::super::settle::{GroupOutcome, SettleError, SettlementAuthority};
    use super::handle;

    /// An authority that fails a chosen account and counts its calls.
    #[derive(Debug)]
    struct Scripted {
        failing: Option<OrganizationId>,
        leaves_pending: bool,
        serialization_failures: AtomicUsize,
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl SettlementAuthority for Scripted {
        async fn probe_role(&self) -> Result<(), SettleError> {
            Ok(())
        }

        async fn settle(
            &self,
            organization: OrganizationId,
            messages: &[Delivered],
        ) -> Result<GroupOutcome, SettleError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.serialization_failures.load(Ordering::Relaxed) > 0 {
                self.serialization_failures.fetch_sub(1, Ordering::Relaxed);
                return Err(SettleError::Serialization);
            }
            if self.failing == Some(organization) {
                return Err(SettleError::OutcomeUnknown("lost commit".to_owned()));
            }
            Ok(GroupOutcome {
                organization,
                claimed: messages.len(),
                duplicates: 0,
                pending: usize::from(self.leaves_pending) * messages.len(),
                quarantined: Vec::new(),
            })
        }
    }

    fn identity(seed: u8) -> aex_wire::Uuid7 {
        aex_wire::Uuid7::from_bytes([
            0x01, 0x93, 0x3f, 0x2a, 0x1c, 0x00, 0x70, 0x00, 0x80, 0x00, 0, 0, 0, 0, 0, seed,
        ])
        .expect("a UUIDv7")
    }

    fn organization(seed: u8) -> OrganizationId {
        OrganizationId::from_uuid7(identity(seed))
    }

    fn fact(organization: OrganizationId, ordinal: u128) -> UsageFact {
        let region = Region::from_name("eu-west-1").expect("a region");
        let authority = FactAuthority {
            kind: AuthorityKind::Compute,
            authority_id: "run_01kyw2qa4pew48j2gb1g6gw3rg".into(),
            segment_ordinal: DecimalU128::new(ordinal),
        };
        UsageFact {
            schema_version: SchemaVersion::V1,
            fact_id: FactId::derive(region, Meter::ComputeMillicpuMs, &authority),
            meter: Meter::ComputeMillicpuMs,
            organization,
            workspace: WorkspaceId::from_uuid7(identity(9)),
            region,
            attribution: Attribution {
                session: None,
                run: None,
                operation: None,
            },
            authority,
            basis: FactBasis::Consumed,
            quantity: DecimalU128::new(1_000),
            service_time: ServiceTime::Instant {
                at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
            },
            source_receipt: SourceReceipt {
                source: "usage-compute-worker".into(),
                receipt_id: "rcp_1".into(),
                observed_at: Timestamp::from_unix_millis(1_800_000_000_000).expect("an instant"),
            },
            pricing_version: PricingVersion("synthetic-zero-v1".to_owned()),
            idempotency: FactIdempotency {
                deduplication_id: "eu-west-1:compute:usage_1".into(),
                business_key: "usage:eu-west-1:compute:usage_1".into(),
            },
        }
    }

    fn message(
        message_id: &str,
        organization: OrganizationId,
        ordinal: u128,
        group: Option<&str>,
    ) -> SqsMessage {
        let body = serde_json::to_string(&RatingRequest {
            fact: fact(organization, ordinal),
            intent_hash: IntentDigest::from_bytes([2u8; 32]),
        })
        .expect("the body encodes");
        raw_message(message_id, &body, group)
    }

    /// A delivered message with an arbitrary body, exactly as Lambda shapes it.
    ///
    /// `SqsMessage` is `#[non_exhaustive]`, so the fixture is decoded from the
    /// exact JSON Lambda delivers rather than built field by field.
    fn raw_message(message_id: &str, body: &str, group: Option<&str>) -> SqsMessage {
        let mut attributes = HashMap::new();
        if let Some(group) = group {
            attributes.insert(MESSAGE_GROUP_ATTRIBUTE.to_owned(), group.to_owned());
        }
        let body = serde_json::to_string(body).expect("the body nests");
        let attributes = serde_json::to_string(&attributes).expect("the attributes encode");
        serde_json::from_str(&format!(
            r#"{{"messageId":"{message_id}","body":{body},"attributes":{attributes},
               "messageAttributes":{{}},"md5OfBody":"","eventSource":"aws:sqs",
               "eventSourceARN":"arn:aws:sqs:eu-west-1:000000000000:aex-dev-usage-rating.fifo",
               "awsRegion":"eu-west-1"}}"#
        ))
        .expect("the delivered message decodes")
    }

    fn event(records: &[SqsMessage]) -> SqsEvent {
        let records = serde_json::to_string(records).expect("the records encode");
        serde_json::from_str(&format!(r#"{{"Records":{records}}}"#))
            .expect("the delivered batch decodes")
    }

    #[tokio::test]
    async fn a_fully_committed_batch_names_no_failure() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[
                message("m1", first, 0, Some(first.encode().as_str())),
                message("m2", first, 1, Some(first.encode().as_str())),
            ]),
            100,
            3,
        )
        .await;
        assert!(response.batch_item_failures.is_empty());
        assert_eq!(report.claimed, 2);
        assert_eq!(report.accounts, 1);
    }

    #[tokio::test]
    async fn a_mixed_batch_names_only_the_uncommitted_account() {
        let first = organization(1);
        let second = organization(2);
        let authority = Arc::new(Scripted {
            failing: Some(second),
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[
                message("m1", first, 0, Some(first.encode().as_str())),
                message("m2", second, 1, Some(second.encode().as_str())),
                message("m3", first, 2, Some(first.encode().as_str())),
            ]),
            100,
            3,
        )
        .await;
        let named: Vec<&str> = response
            .batch_item_failures
            .iter()
            .map(|failure| failure.item_identifier.as_str())
            .collect();
        assert_eq!(
            named,
            vec!["m2"],
            "a committed account is never redelivered"
        );
        assert_eq!(report.failed_accounts, 1);
        assert_eq!(report.claimed, 2);
    }

    #[tokio::test]
    async fn a_serialization_race_is_retried_and_an_unknown_outcome_is_not() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(2),
            calls: AtomicUsize::new(0),
        });
        let (response, _) = handle(
            &authority,
            event(&[message("m1", first, 0, Some(first.encode().as_str()))]),
            100,
            3,
        )
        .await;
        assert!(
            response.batch_item_failures.is_empty(),
            "a bounded retry converges"
        );
        assert_eq!(authority.calls.load(Ordering::Relaxed), 3);

        let unknown = Arc::new(Scripted {
            failing: Some(first),
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, _) = handle(
            &unknown,
            event(&[message("m1", first, 0, Some(first.encode().as_str()))]),
            100,
            3,
        )
        .await;
        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(
            unknown.calls.load(Ordering::Relaxed),
            1,
            "an unknown outcome is never retried in place"
        );
    }

    #[tokio::test]
    async fn a_message_with_no_fifo_group_is_returned_rather_than_dropped() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, _) =
            handle(&authority, event(&[message("m1", first, 0, None)]), 100, 3).await;
        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(response.batch_item_failures[0].item_identifier, "m1");
    }

    #[tokio::test]
    async fn one_account_settles_in_contiguous_chunks_rather_than_one_huge_transaction() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[
                message("m1", first, 0, Some(first.encode().as_str())),
                message("m2", first, 1, Some(first.encode().as_str())),
                message("m3", first, 2, Some(first.encode().as_str())),
            ]),
            2,
            3,
        )
        .await;
        assert!(response.batch_item_failures.is_empty());
        assert_eq!(report.claimed, 3);
        assert_eq!(
            authority.calls.load(Ordering::Relaxed),
            2,
            "three messages at two per transaction is two transactions"
        );
    }

    #[tokio::test]
    async fn a_durable_pending_claim_is_redelivered_until_a_receipt_exists() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: true,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[message("m1", first, 0, Some(first.encode().as_str()))]),
            100,
            3,
        )
        .await;
        assert_eq!(response.batch_item_failures.len(), 1);
        assert_eq!(response.batch_item_failures[0].item_identifier, "m1");
        assert_eq!(report.pending, 1);
        assert_eq!(report.failed_accounts, 1);
    }

    #[tokio::test]
    async fn one_undecodable_body_fails_its_own_group_tail_and_nothing_else() {
        // The typed event layer used to fail the whole batch into the DLQ on
        // one bad body. Decoding per record must keep other accounts settling,
        // while the poisoned account's tail stays queued: FIFO per account is
        // the ledger-ordering guarantee, so its later facts must not settle
        // ahead of the one that failed.
        let healthy = organization(1);
        let poisoned = organization(2);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[
                message("m1", healthy, 0, Some(healthy.encode().as_str())),
                raw_message(
                    "m2",
                    "{\"not\":\"a rating request\"}",
                    Some(poisoned.encode().as_str()),
                ),
                message("m3", poisoned, 1, Some(poisoned.encode().as_str())),
                message("m4", healthy, 2, Some(healthy.encode().as_str())),
            ]),
            100,
            3,
        )
        .await;
        let named: Vec<&str> = response
            .batch_item_failures
            .iter()
            .map(|failure| failure.item_identifier.as_str())
            .collect();
        assert_eq!(
            named,
            vec!["m2", "m3"],
            "the poisoned body and its group tail return; the healthy account settles"
        );
        assert_eq!(report.claimed, 2);
        assert_eq!(report.undecodable, 2);
        assert_eq!(report.accounts, 1);
    }

    #[tokio::test]
    async fn a_batch_that_cannot_be_grouped_returns_every_delivered_message() {
        // A partial-batch response acknowledges everything it does not name.
        // When the grouping itself cannot be trusted, "return the batch whole"
        // must therefore name every delivered message, or the refusal silently
        // deletes money evidence.
        let first = organization(1);
        let second = organization(2);
        let authority = Arc::new(Scripted {
            failing: None,
            leaves_pending: false,
            serialization_failures: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        });
        let (response, report) = handle(
            &authority,
            event(&[
                message("m1", first, 0, Some(first.encode().as_str())),
                // Declares the wrong account for its fact, which refuses the
                // whole grouping.
                message("m2", second, 1, Some(first.encode().as_str())),
            ]),
            100,
            3,
        )
        .await;
        let named: Vec<&str> = response
            .batch_item_failures
            .iter()
            .map(|failure| failure.item_identifier.as_str())
            .collect();
        assert_eq!(named, vec!["m1", "m2"]);
        assert_eq!(
            authority.calls.load(Ordering::Relaxed),
            0,
            "nothing settles out of a batch whose ordering cannot be trusted"
        );
        assert_eq!(report.undecodable, 2);
    }
}
