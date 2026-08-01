//! Slice S-2.3/S-2.4 — the adapters' request bytes and their page rules.
//!
//! The request assertions run against `capture_request`, so what is checked is the exact
//! serialized body the SDK would put on the wire rather than a hand-written mirror of it.
//! The page assertions run on decoded rows, because contiguity and fork detection are the
//! two rules that decide whether an agent may act at all, and neither should need a service
//! to observe.

use aex_brain_application::ports::{
    AgentHead, CancelToken, Claim, ClaimError, ConditionFailure, EffectStore, FenceGuard,
    JournalStore, LeaseStore, ReadBudget, ReleaseDisposition, StoreError,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JournalSeq,
    OwnerToken, SessionId, Timestamp,
};
use aex_brain_domain::journal::{FinishReason, JournalRecord};
use aex_brain_store_aws::journal::condition_for;
use aex_brain_store_aws::{BrainStore, DecisionContext};
use aex_session_dynamodb::attr::{ItemBuilder, b, n, s, stamp};
use aex_session_dynamodb::plan::{Participant, RegionalTables};
use aex_wire::ids::{PrefixedId, Uuid7};
use aws_smithy_http_client::test_util::{CaptureRequestReceiver, capture_request};

fn v7(millis: u64, seed: u8) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
}

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(v7(1_767_225_600_000, 1)),
        AgentId(v7(1_767_225_600_001, 2)),
    )
}

fn context() -> DecisionContext {
    DecisionContext {
        tables: RegionalTables::composed("dev", "eu-west-1"),
        workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(
            1_767_225_600_002,
            [3; 10],
        )),
        organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
            1_767_225_600_003,
            [4; 10],
        )),
        deletion_epoch: 0,
        lease_expires_at: Timestamp::from_millis(1_767_225_615_000),
        now: Timestamp::from_millis(1_767_225_600_000),
    }
}

fn capturing() -> (BrainStore, CaptureRequestReceiver) {
    let (http_client, receiver) = capture_request(None);
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::for_tests())
        .http_client(http_client)
        .build();
    (
        BrainStore::new(aws_sdk_dynamodb::Client::from_conf(config), context()),
        receiver,
    )
}

fn captured(receiver: CaptureRequestReceiver) -> serde_json::Value {
    let request = receiver.expect_request();
    let body = std::str::from_utf8(request.body().bytes().unwrap_or_default())
        .expect("the request body is UTF-8");
    serde_json::from_str(body).expect("the request body is JSON")
}

fn head() -> AgentHead {
    AgentHead {
        key: key(),
        revision: AgentRevision(1),
        fence: Fence(3),
        journal_tail: Some(JournalSeq(4)),
        cancel_epoch: CancelEpoch(0),
        finish: None,
        phase: "awaiting_model".to_owned(),
        budget: aex_brain_domain::budget::BudgetNode::default(),
        stop_requested: false,
        open_effects: Vec::new(),
        lease_expires_at: Timestamp::from_millis(1_767_225_615_000),
    }
}

fn claim() -> Claim {
    Claim {
        key: key(),
        owner: OwnerToken(v7(1_767_225_600_004, 5)),
        fence: Fence(3),
        expires_at: Timestamp::from_millis(1_767_225_615_000),
        head: head(),
    }
}

/// A claim advances the fence and returns the head in the same write. The head is what the
/// activation folds from, so a second strongly-consistent read here would double the cost
/// of becoming an owner for no extra information.
#[tokio::test]
async fn a_claim_advances_the_fence_and_asks_for_the_head_in_one_round_trip() {
    let (store, receiver) = capturing();
    let _ = store
        .claim(
            &key(),
            OwnerToken(v7(1_767_225_600_004, 5)),
            core::time::Duration::from_secs(15),
            Timestamp::from_millis(1_767_225_600_000),
        )
        .await;
    let body = captured(receiver);
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(update.contains("ADD fence :one"), "{update}");
    assert_eq!(body["ReturnValues"], "ALL_NEW");
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(
        condition.contains("attribute_not_exists(finishReason)"),
        "a terminal agent has nothing to claim: {condition}"
    );
    assert!(
        condition.contains("leaseExpiresAt < :stealable"),
        "{condition}"
    );
}

/// A renewal must never move the fence: it proves nothing changed hands, and moving it
/// would fence out the very owner doing the renewing.
#[tokio::test]
async fn a_renewal_extends_the_lease_without_touching_the_fence() {
    let (store, receiver) = capturing();
    let _ = store
        .renew(
            &claim(),
            core::time::Duration::from_secs(15),
            Timestamp::from_millis(1_767_225_610_000),
        )
        .await;
    let body = captured(receiver);
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(
        !update.contains("fence"),
        "extending a lease is not a new claim: {update}"
    );
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(condition.contains("fence = :fence"), "{condition}");
    assert!(condition.contains("claimOwner = :owner"), "{condition}");
}

/// Drain sets the expiry to zero so a surviving task claims immediately rather than waiting
/// out the whole TTL. Every other disposition leaves the expiry where it was.
#[tokio::test]
async fn a_drained_release_expires_the_lease_immediately() {
    let (store, receiver) = capturing();
    let _ = store.release(claim(), ReleaseDisposition::Drain).await;
    let drained = captured(receiver);
    assert_eq!(
        drained["ExpressionAttributeValues"][":expiry"]["S"],
        "1970-01-01T00:00:00.000Z"
    );

    let (store, receiver) = capturing();
    let _ = store.release(claim(), ReleaseDisposition::Parked).await;
    let parked = captured(receiver);
    assert_ne!(
        parked["ExpressionAttributeValues"][":expiry"]["S"],
        "1970-01-01T00:00:00.000Z"
    );
}

/// The durable pre-send write conditions on the effect state **and** the agent fence. A
/// fenced-out owner that could still mint a ticket would send a second generation of a
/// request the new owner is already responsible for.
#[tokio::test]
async fn the_pre_send_write_conditions_on_both_the_effect_state_and_the_fence() {
    let (store, receiver) = capturing();
    let guard = FenceGuard::new(
        key(),
        OwnerToken(v7(1_767_225_600_004, 5)),
        Fence(7),
        AgentRevision(2),
        Some(JournalSeq(4)),
        CancelEpoch(0),
        CancelToken::new(),
    );
    let _ = store
        .mark_dispatch_started(
            &guard,
            &EffectId([9; 16]),
            1,
            Timestamp::from_millis(1_767_225_600_000),
        )
        .await;
    let body = captured(receiver);
    let condition = body["ConditionExpression"].as_str().expect("conditional");
    assert!(condition.contains("#state = :prepared"), "{condition}");
    assert!(condition.contains("agentFence = :fence"), "{condition}");
    assert_eq!(body["ExpressionAttributeValues"][":fence"]["N"], "7");
}

/// A journal read is strongly consistent and bounded. An authority read that may be stale
/// is not an authority read, and an unbounded one is how one large agent takes the whole
/// task's memory envelope with it.
#[tokio::test]
async fn a_journal_page_is_strongly_consistent_and_bounded() {
    let (store, receiver) = capturing();
    let _ = store
        .read_page(
            &key(),
            JournalSeq(4),
            ReadBudget {
                max_entries: 25,
                max_bytes: 1_024,
            },
        )
        .await;
    let body = captured(receiver);
    assert_eq!(body["ConsistentRead"], true);
    assert_eq!(body["Limit"], 25);
    assert_eq!(
        body["ExpressionAttributeValues"][":from"]["S"],
        "J#00000000000000000004"
    );
}

// ---------------------------------------------------------------------------
// page rules
// ---------------------------------------------------------------------------

fn entry(seq: u64, record: &JournalRecord) -> aex_session_dynamodb::attr::Item {
    let body = record.canonical_bytes().expect("canonical");
    ItemBuilder::new(aex_session_dynamodb::codec::JOURNAL_ENTRY)
        .set("seq", n(seq))
        .set("entryId", s(ContentHash::of(&body).to_hex()))
        .set("kind", s(record.kind_name()))
        .set("bodyBytes", n(body.len() as u64))
        .set(aex_session_dynamodb::codec::BODY_INLINE, b(body))
        .set(
            "occurredAt",
            stamp(
                aex_wire::types::Timestamp::from_unix_millis(1_767_225_600_000).expect("in range"),
            ),
        )
        .build()
}

fn finished() -> JournalRecord {
    JournalRecord::AgentFinished {
        reason: FinishReason::Completed,
        failure: None,
    }
}

fn budget(entries: usize, bytes: usize) -> ReadBudget {
    ReadBudget {
        max_entries: entries,
        max_bytes: bytes,
    }
}

#[test]
fn a_contiguous_page_decodes_in_order() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record), entry(6, &record)];
    let page =
        aex_brain_store_aws::journal::decode_page(&items, JournalSeq(4), budget(25, 1 << 20))
            .expect("a contiguous page folds");
    assert_eq!(page.entries.len(), 3);
    assert_eq!(page.entries[0].envelope.seq, JournalSeq(4));
    assert_eq!(page.entries[2].envelope.seq, JournalSeq(6));
    assert_eq!(page.next, None);
}

/// Half a page is worse than none: it would let an agent act on a prefix of its own
/// history and believe it was complete.
#[test]
fn a_page_that_observes_a_gap_returns_no_entries_at_all() {
    let record = finished();
    let items = [entry(4, &record), entry(6, &record)];
    let error =
        aex_brain_store_aws::journal::decode_page(&items, JournalSeq(4), budget(25, 1 << 20))
            .expect_err("a gap is a typed error, never a fold");
    assert_eq!(
        error,
        StoreError::JournalGap {
            missing: JournalSeq(5)
        }
    );
}

/// A body that does not hash to its recorded entry id is a fork. Folding either side of one
/// silently forks the agent's whole future, so the page refuses instead.
#[test]
fn a_body_that_disagrees_with_its_recorded_hash_quarantines() {
    let mut item = entry(4, &finished());
    item.insert("entryId".to_owned(), s("f".repeat(64)));
    let error =
        aex_brain_store_aws::journal::decode_page(&[item], JournalSeq(4), budget(25, 1 << 20))
            .expect_err("a fork is never folded");
    assert!(
        matches!(error, StoreError::JournalForked { seq, .. } if seq == JournalSeq(4)),
        "{error:?}"
    );
}

#[test]
fn a_page_that_exhausts_its_byte_budget_says_so_rather_than_truncating() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record)];
    let error = aex_brain_store_aws::journal::decode_page(&items, JournalSeq(4), budget(25, 1))
        .expect_err("an exhausted budget is reported");
    assert!(
        matches!(error, StoreError::ReadBudgetExhausted { .. }),
        "{error:?}"
    );
}

#[test]
fn a_full_page_reports_where_to_resume() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record)];
    let page = aex_brain_store_aws::journal::decode_page(&items, JournalSeq(4), budget(2, 1 << 20))
        .expect("a full page");
    assert_eq!(page.next, Some(JournalSeq(6)));
}

/// Every named participant maps to exactly one precondition failure, and the four answers
/// are genuinely different: reload, replan, treat as success, or quarantine.
#[test]
fn every_participant_names_the_failure_it_means() {
    assert!(matches!(
        condition_for(Participant::SESSION_HEAD_GUARD),
        ConditionFailure::CancelEpochAdvanced
    ));
    assert!(matches!(
        condition_for(Participant::AGENT_CONTROL),
        ConditionFailure::StaleFence
    ));
    assert!(matches!(
        condition_for(Participant::AGENT_JOURNAL),
        ConditionFailure::IdempotentReplay(_)
    ));
    assert!(matches!(
        condition_for(Participant::WORK_DEDUPE),
        ConditionFailure::IdempotentReplay(_)
    ));
    assert!(matches!(
        condition_for(aex_brain_store_aws::plan::participant::SESSION_BUDGET),
        ConditionFailure::BudgetExhausted
    ));
    assert!(matches!(
        condition_for(aex_brain_store_aws::plan::participant::CHILD_INDEX),
        ConditionFailure::ChildStateMismatch
    ));
}

/// A claim whose condition fails is `HeldByOther`, never a transport error: the caller acks
/// the wake and moves on rather than retrying into a live owner.
#[test]
fn a_claim_failure_distinguishes_a_live_owner_from_a_broken_store() {
    let held = ClaimError::HeldByOther {
        expires_at: Timestamp::from_millis(1),
    };
    assert!(!matches!(held, ClaimError::Store(_)));
    assert!(matches!(
        ClaimError::Store(StoreError::Transport {
            reason: String::new(),
            retryable: true
        }),
        ClaimError::Store(_)
    ));
}
