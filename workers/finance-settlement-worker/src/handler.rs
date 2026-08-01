//! One SQS invocation, from the delivered batch to the partial-batch response.

use std::sync::Arc;

use aex_finance_app::use_cases::RatingRequest;
use aex_wire::ids::OrganizationId;
use aws_lambda_events::sqs::{SqsBatchResponse, SqsEventObj};

use crate::backlog::{Delivered, MESSAGE_GROUP_ATTRIBUTE, chunks, partition, uncommitted};
use crate::settle::{SettleError, SettlementAuthority};

/// What one invocation observed, for the record it emits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReport {
    /// How many accounts the batch touched.
    pub accounts: usize,
    /// How many facts became durable for the first time.
    pub claimed: usize,
    /// How many facts a duplicate delivery had already stored.
    pub duplicates: usize,
    /// How many facts were quarantined on an intent conflict.
    pub quarantined: usize,
    /// How many accounts did not commit.
    pub failed_accounts: usize,
}

/// Decodes one delivered SQS batch.
///
/// A message with no identity or no FIFO group is not silently dropped: it is
/// returned as a failure so it goes back to the queue and is visible.
#[must_use]
pub fn decode(event: SqsEventObj<RatingRequest>) -> (Vec<Delivered>, Vec<String>) {
    let mut delivered = Vec::with_capacity(event.records.len());
    let mut undecodable = Vec::new();
    for record in event.records {
        let Some(message_id) = record.message_id.clone() else {
            continue;
        };
        let group = record
            .attributes
            .get(MESSAGE_GROUP_ATTRIBUTE)
            .cloned()
            .unwrap_or_default();
        if group.is_empty() {
            undecodable.push(message_id);
            continue;
        }
        delivered.push(Delivered {
            message_id,
            group,
            request: record.body,
        });
    }
    (delivered, undecodable)
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
    event: SqsEventObj<RatingRequest>,
    max_group_batch: u32,
    serialization_retry_max: u32,
) -> (SqsBatchResponse, BatchReport) {
    let mut response = SqsBatchResponse::default();
    let mut report = BatchReport::default();

    let (delivered, undecodable) = decode(event);
    for message_id in undecodable {
        response.add_failure(message_id);
    }

    // A batch whose grouping cannot be trusted is returned whole: the ordering
    // guarantee is the reason this queue is FIFO at all.
    let Ok(groups) = partition(delivered) else {
        return (response, report);
    };
    report.accounts = groups.len();

    let mut failed: Vec<OrganizationId> = Vec::new();
    for group in &groups {
        let mut committed = true;
        for chunk in chunks(group, max_group_batch) {
            let settled = settle_with_retry(
                authority,
                group.organization,
                &chunk,
                serialization_retry_max,
            )
            .await;
            let Ok(outcome) = settled else {
                committed = false;
                break;
            };
            report.claimed += outcome.claimed;
            report.duplicates += outcome.duplicates;
            report.quarantined += outcome.quarantined.len();
        }
        if !committed {
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
) -> Result<crate::settle::GroupOutcome, SettleError> {
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
    use aex_finance_domain::IntentHash;
    use aex_internal_contracts::usage::{
        Attribution, AuthorityKind, FactAuthority, FactBasis, FactId, FactIdempotency, Meter,
        ServiceTime, SourceReceipt, UsageFact,
    };
    use aex_internal_contracts::{PricingVersion, SchemaVersion};
    use aex_wire::PrefixedId as _;
    use aex_wire::ids::{OrganizationId, WorkspaceId};
    use aex_wire::types::{DecimalU128, Region, Timestamp};
    use aws_lambda_events::sqs::{SqsEventObj, SqsMessageObj};

    use super::handle;
    use crate::backlog::{Delivered, MESSAGE_GROUP_ATTRIBUTE};
    use crate::settle::{GroupOutcome, SettleError, SettlementAuthority};

    /// An authority that fails a chosen account and counts its calls.
    #[derive(Debug)]
    struct Scripted {
        failing: Option<OrganizationId>,
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
    ) -> SqsMessageObj<RatingRequest> {
        let mut attributes = HashMap::new();
        if let Some(group) = group {
            attributes.insert(MESSAGE_GROUP_ATTRIBUTE.to_owned(), group.to_owned());
        }
        // `SqsMessageObj` is `#[non_exhaustive]`, so the fixture is decoded
        // from the exact JSON Lambda delivers rather than built field by field.
        // SQS delivers the body as a JSON *string*, which the typed record then
        // parses, so the fixture nests it exactly the way Lambda does.
        let body = serde_json::to_string(
            &serde_json::to_string(&RatingRequest {
                fact: fact(organization, ordinal),
                intent_hash: IntentHash::new([2u8; 32]),
            })
            .expect("the body encodes"),
        )
        .expect("the body nests");
        let attributes = serde_json::to_string(&attributes).expect("the attributes encode");
        serde_json::from_str(&format!(
            r#"{{"messageId":"{message_id}","body":{body},"attributes":{attributes},
               "messageAttributes":{{}},"md5OfBody":"","eventSource":"aws:sqs",
               "eventSourceARN":"arn:aws:sqs:eu-west-1:000000000000:aex-dev-usage-rating.fifo",
               "awsRegion":"eu-west-1"}}"#
        ))
        .expect("the delivered message decodes")
    }

    fn event(records: &[SqsMessageObj<RatingRequest>]) -> SqsEventObj<RatingRequest> {
        let records = serde_json::to_string(records).expect("the records encode");
        serde_json::from_str(&format!(r#"{{"Records":{records}}}"#))
            .expect("the delivered batch decodes")
    }

    #[tokio::test]
    async fn a_fully_committed_batch_names_no_failure() {
        let first = organization(1);
        let authority = Arc::new(Scripted {
            failing: None,
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
}
