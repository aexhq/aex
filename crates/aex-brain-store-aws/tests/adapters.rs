//! Slice S-2.3/S-2.4 — the adapters' request bytes and their page rules.
//!
//! The request assertions run against `capture_request`, so what is checked is the exact
//! serialized body the SDK would put on the wire rather than a hand-written mirror of it.
//! The page assertions run on decoded rows, because contiguity and fork detection are the
//! two rules that decide whether an agent may act at all, and neither should need a service
//! to observe.

use aex_brain_application::ports::{
    AgentHead, CancelToken, Claim, ClaimError, ConditionFailure, DispatchTicket, DueScanCursor,
    DurableWake, EffectStore, FenceGuard, JournalCursor, JournalStore, LeaseStore, ReadBudget,
    ReleaseDisposition, SessionAuthority, StoreError, WakeQueue,
};
use aex_brain_domain::ids::{
    AgentId, AgentKey, AgentRevision, CancelEpoch, ContentHash, EffectId, Fence, JournalSeq,
    OwnerToken, SessionId, Timestamp, WakeId,
};
use aex_brain_domain::journal::{FinishReason, JournalRecord, ParkReason};
use aex_brain_store_aws::journal::{condition_for, dispatch_order};
use aex_brain_store_aws::{BrainStore, BrainTables, DueScan, SqsWakeQueue};
use aex_session_dynamodb::attr::{ItemBuilder, b, n, s, stamp};
use aex_session_dynamodb::plan::Participant;
use aex_wire::ids::{GenerationId, PrefixedId, Uuid7};
use aws_smithy_http_client::test_util::{
    CaptureRequestReceiver, ReplayEvent, StaticReplayClient, capture_request,
};
use aws_smithy_types::body::SdkBody;
use base64::Engine as _;

#[path = "adapters/snapshots.rs"]
mod snapshots;

fn v7(millis: u64, seed: u8) -> uuid::Uuid {
    uuid::Uuid::from_bytes(*Uuid7::compose(millis, [seed; 10]).as_bytes())
}

fn key() -> AgentKey {
    AgentKey::new(
        SessionId(v7(1_767_225_600_000, 1)),
        AgentId(v7(1_767_225_600_001, 2)),
    )
}

fn authority() -> SessionAuthority {
    SessionAuthority {
        workspace: aex_wire::ids::WorkspaceId::from_uuid7(Uuid7::compose(
            1_767_225_600_002,
            [3; 10],
        )),
        organization: aex_wire::ids::OrganizationId::from_uuid7(Uuid7::compose(
            1_767_225_600_003,
            [4; 10],
        )),
        deletion_epoch: 0,
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
        BrainStore::new(
            aws_sdk_dynamodb::Client::from_conf(config),
            BrainTables {
                session_authority: "dev-eu-west-1-session-authority".to_owned(),
                regional_work: "dev-eu-west-1-regional-work".to_owned(),
            },
        ),
        receiver,
    )
}

fn replaying(responses: Vec<serde_json::Value>) -> (BrainStore, StaticReplayClient) {
    let events = responses
        .into_iter()
        .map(|response| {
            ReplayEvent::new(
                http::Request::builder()
                    .method("POST")
                    .uri("https://dynamodb.eu-west-1.amazonaws.com/")
                    .body(SdkBody::empty())
                    .expect("a request"),
                http::Response::builder()
                    .status(200)
                    .body(SdkBody::from(response.to_string()))
                    .expect("a response"),
            )
        })
        .collect();
    let replay = StaticReplayClient::new(events);
    let config = aws_sdk_dynamodb::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::for_tests())
        .http_client(replay.clone())
        .retry_config(aws_sdk_dynamodb::config::retry::RetryConfig::disabled())
        .build();
    (
        BrainStore::new(
            aws_sdk_dynamodb::Client::from_conf(config),
            BrainTables {
                session_authority: "dev-eu-west-1-session-authority".to_owned(),
                regional_work: "dev-eu-west-1-regional-work".to_owned(),
            },
        ),
        replay,
    )
}

fn capturing_wake_queue() -> (SqsWakeQueue, CaptureRequestReceiver) {
    let (dynamo_http, receiver) = capture_request(None);
    let dynamo = aws_sdk_dynamodb::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_dynamodb::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_dynamodb::config::Credentials::for_tests())
        .http_client(dynamo_http)
        .build();
    let (sqs_http, _unused) = capture_request(None);
    let sqs = aws_sdk_sqs::Config::builder()
        .behavior_version_latest()
        .region(aws_sdk_sqs::config::Region::new("eu-west-1"))
        .credentials_provider(aws_sdk_sqs::config::Credentials::for_tests())
        .http_client(sqs_http)
        .build();
    (
        SqsWakeQueue::new(
            aws_sdk_sqs::Client::from_conf(sqs),
            "https://sqs.eu-west-1.amazonaws.com/000000000000/brain-wake".to_owned(),
            DueScan::new(
                aws_sdk_dynamodb::Client::from_conf(dynamo),
                "dev-eu-west-1-regional-work".to_owned(),
            ),
        ),
        receiver,
    )
}

fn captured(receiver: CaptureRequestReceiver) -> serde_json::Value {
    let request = receiver.expect_request();
    let body = std::str::from_utf8(request.body().bytes().unwrap_or_default())
        .expect("the request body is UTF-8");
    serde_json::from_str(body).expect("the request body is JSON")
}

fn captured_requests(replay: &StaticReplayClient) -> Vec<serde_json::Value> {
    replay
        .actual_requests()
        .map(|request| {
            serde_json::from_slice(
                request
                    .body()
                    .bytes()
                    .expect("the DynamoDB request body is in memory"),
            )
            .expect("the DynamoDB request body is JSON")
        })
        .collect()
}

fn head() -> AgentHead {
    AgentHead {
        key: key(),
        generation: GenerationId::from_uuid7(Uuid7::compose(1_767_225_600_000, [9; 10])),
        revision: AgentRevision(1),
        fence: Fence(3),
        journal_tail: Some(JournalSeq(4)),
        journal_tail_hash: Some(ContentHash::of(b"tail")),
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
        authority: authority(),
        head: head(),
    }
}

#[tokio::test]
async fn a_delivery_hint_is_verified_by_a_strong_source_row_read() {
    let (queue, receiver) = capturing_wake_queue();
    let _ = queue
        .state(&DurableWake {
            id: WakeId(v7(1_767_225_600_006, 7)),
            work_id: "wrk_01".to_owned(),
            key: key(),
            dedup_key: "wrk_01".to_owned(),
            reason: ParkReason::AwaitingUserMessage,
            due: Some(Timestamp::from_millis(1_767_225_600_000)),
            priority: 1,
            tenant: authority().workspace.to_string(),
        })
        .await;
    let body = captured(receiver);
    assert_eq!(body["TableName"], "dev-eu-west-1-regional-work");
    assert_eq!(body["ConsistentRead"], true);
    assert_eq!(body["Key"]["pk"]["S"], "WORK#wrk_01");
    assert_eq!(body["Key"]["sk"]["S"], "STATE");
}

/// A claim advances the fence and returns the agent head in the same write. Session authority
/// is a different item and is read strongly consistently after this write succeeds.
#[tokio::test]
async fn a_claim_advances_the_fence_and_returns_the_agent_head_with_the_write() {
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
        !condition.contains("finishReason"),
        "a terminal agent remains claimable so its delivered source can retire under a fence: {condition}"
    );
    assert!(
        condition.contains("leaseExpiresAt < :stealable"),
        "{condition}"
    );
}

/// A renewal must never move the fence, and it must observe session lifecycle, cancellation
/// and deletion in the same transaction that extends the agent lease.
#[tokio::test]
async fn a_renewal_guards_session_authority_without_touching_the_fence() {
    let (store, receiver) = capturing();
    let _ = store
        .renew(
            &claim(),
            core::time::Duration::from_secs(15),
            Timestamp::from_millis(1_767_225_610_000),
        )
        .await;
    let body = captured(receiver);
    let actions = body["TransactItems"]
        .as_array()
        .expect("renewal is a transaction");
    assert_eq!(actions.len(), 2);
    let session = &actions[0]["ConditionCheck"];
    let session_condition = session["ConditionExpression"]
        .as_str()
        .expect("a session condition");
    for required in [
        "cancelEpoch = :cancelEpoch",
        "deletionEpoch = :deletionEpoch",
        "lifecycle = :active",
    ] {
        assert!(session_condition.contains(required), "{session_condition}");
    }
    let control = &actions[1]["Update"];
    let update = control["UpdateExpression"].as_str().expect("an update");
    assert!(
        !update.contains("fence"),
        "extending a lease is not a new claim: {update}"
    );
    let condition = control["ConditionExpression"]
        .as_str()
        .expect("conditional");
    assert!(condition.contains("fence = :fence"), "{condition}");
    assert!(condition.contains("claimOwner = :owner"), "{condition}");
    assert!(
        condition.contains("cancelEpoch = :cancelEpoch"),
        "{condition}"
    );
}

/// Every completed ownership scope gives the lease back immediately. Removing the owner
/// while retaining a future expiry creates an ownerless interval in which nobody can claim.
#[tokio::test]
async fn every_release_disposition_expires_the_exact_owned_lease_immediately() {
    for disposition in [
        ReleaseDisposition::Committed,
        ReleaseDisposition::Parked,
        ReleaseDisposition::Drain,
        ReleaseDisposition::Abandoned,
    ] {
        let (store, receiver) = capturing();
        let _ = store.release(claim(), disposition).await;
        let released = captured(receiver);
        let update = released["UpdateExpression"]
            .as_str()
            .expect("a release update");
        assert!(
            update.contains("REMOVE claimOwner, leaseExpiresAt"),
            "{update}"
        );
        assert!(
            released["ExpressionAttributeValues"][":expiry"].is_null(),
            "{disposition:?} retained an expiry value"
        );
        let condition = released["ConditionExpression"]
            .as_str()
            .expect("a conditional release");
        assert!(condition.contains("fence = :fence"), "{condition}");
        assert!(condition.contains("claimOwner = :owner"), "{condition}");
    }
}

/// The durable pre-send transition is a three-item transaction: current session lifecycle
/// and epochs plus current agent ownership are checked independently of the fence stamped
/// when the effect was prepared, and the effect is then taken over under the current fence.
/// This rejects cancellation/deletion and a stale owner while allowing a successor to
/// dispatch the same prepared identity.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "the transaction-shape test audits every participant and condition together"
)]
async fn the_pre_send_write_atomically_checks_current_control_and_takes_over_the_effect() {
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
            &authority(),
            &EffectId([9; 16]),
            1,
            Timestamp::from_millis(1_767_225_600_000),
        )
        .await;
    let body = captured(receiver);
    let actions = body["TransactItems"]
        .as_array()
        .expect("a transaction action list");
    assert_eq!(actions.len(), 3, "{body}");
    assert_eq!(
        dispatch_order(),
        &[
            Participant::SESSION_HEAD_GUARD,
            Participant::AGENT_CONTROL,
            Participant::AGENT_EFFECT,
        ]
    );

    let session = &actions[0]["ConditionCheck"];
    let session_condition = session["ConditionExpression"]
        .as_str()
        .expect("a session-head condition");
    let wire_session =
        aex_wire::ids::SessionId::from_uuid7(Uuid7::compose(1_767_225_600_000, [1; 10]));
    assert_eq!(session["Key"]["pk"]["S"], format!("SESSION#{wire_session}"));
    assert_eq!(session["Key"]["sk"]["S"], "HEAD");
    for required in [
        "cancelEpoch = :cancelEpoch",
        "deletionEpoch = :deletionEpoch",
        "workspaceId = :workspaceId",
        "organizationId = :organizationId",
        "lifecycle = :active",
    ] {
        assert!(session_condition.contains(required), "{session_condition}");
    }
    assert_eq!(
        session["ExpressionAttributeValues"][":cancelEpoch"]["N"],
        "0"
    );
    assert_eq!(
        session["ExpressionAttributeValues"][":deletionEpoch"]["N"],
        "0"
    );
    assert_eq!(
        session["ExpressionAttributeValues"][":workspaceId"]["S"],
        authority().workspace.to_string()
    );
    assert_eq!(
        session["ExpressionAttributeValues"][":organizationId"]["S"],
        authority().organization.to_string()
    );
    assert_eq!(
        session["ExpressionAttributeValues"][":active"]["S"],
        "active"
    );

    let control = &actions[1]["ConditionCheck"];
    let control_condition = control["ConditionExpression"]
        .as_str()
        .expect("a control condition");
    assert!(
        control_condition.contains("fence = :fence"),
        "{control_condition}"
    );
    assert!(
        control_condition.contains("claimOwner = :owner"),
        "{control_condition}"
    );
    assert_eq!(control["ExpressionAttributeValues"][":fence"]["N"], "7");
    assert_eq!(
        control["ExpressionAttributeValues"][":owner"]["S"],
        v7(1_767_225_600_004, 5).as_hyphenated().to_string()
    );

    let effect = &actions[2]["Update"];
    let effect_condition = effect["ConditionExpression"]
        .as_str()
        .expect("an effect condition");
    assert!(
        effect_condition.contains("#state = :prepared"),
        "{effect_condition}"
    );
    assert!(
        effect_condition.contains("attempt = :attempt"),
        "{effect_condition}"
    );
    assert!(
        !effect_condition.contains("agentFence"),
        "the predecessor's prepare fence must not prevent takeover: {effect_condition}"
    );
    let update = effect["UpdateExpression"].as_str().expect("an update");
    assert!(update.contains("agentFence = :fence"), "{update}");
    assert!(
        !update.contains("attempt = :attempt"),
        "takeover rewrote the immutable prepared attempt: {update}"
    );
}

/// Once the application classifies a page stable and installs its cursor, the adapter must
/// return that cursor verbatim as `DynamoDB`'s native `ExclusiveStartKey`. Transient pages
/// retain their previous cursor in the application and never reach this adapter assertion.
#[tokio::test]
async fn a_due_scan_resumes_from_the_native_per_shard_continuation() {
    let (queue, receiver) = capturing_wake_queue();
    let cursor = DueScanCursor::new([
        ("pk", "WORK#wrk_10"),
        ("sk", "STATE"),
        ("dueShardPk", "DUE#0007"),
        ("dueShardSk", "2026-01-01T00:00:00.000Z#wrk_10"),
    ]);
    let _ = queue
        .due_scan(
            aex_brain_domain::ids::WorkShard(7),
            Timestamp::from_millis(1_767_225_600_000),
            10,
            Some(cursor),
        )
        .await;
    let body = captured(receiver);
    assert_eq!(body["ExclusiveStartKey"]["pk"]["S"], "WORK#wrk_10");
    assert_eq!(body["ExclusiveStartKey"]["sk"]["S"], "STATE");
    assert_eq!(body["ExclusiveStartKey"]["dueShardPk"]["S"], "DUE#0007");
    assert_eq!(
        body["ExclusiveStartKey"]["dueShardSk"]["S"],
        "2026-01-01T00:00:00.000Z#wrk_10"
    );
}

/// Every attribute the decoder reads back is written. A detached id and executor are one
/// closed binding: without the route a restarted mux cannot address the accepting backend.
#[tokio::test]
async fn a_response_start_records_the_evidence_the_decoder_reads_back() {
    use aex_brain_domain::effect::{
        DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage,
    };
    use aex_brain_domain::ids::{ContentHash, DetachedOperationId, ProviderRequestId};
    use aex_brain_domain::journal::ExecutorRoute;

    let (store, receiver) = capturing();
    let guard = FenceGuard::new(
        key(),
        OwnerToken(uuid::Uuid::from_u128(5)),
        Fence(7),
        AgentRevision(2),
        Some(JournalSeq(4)),
        CancelEpoch(0),
        CancelToken::new(),
    );
    let ticket = DispatchTicket::mint(
        &guard,
        authority().workspace,
        authority().organization,
        EffectId([9; 16]),
        1,
        Timestamp::from_millis(1_767_225_600_000),
    );
    let receipt = ContentHash::of(b"response");
    let _ = store
        .mark_response_started(
            &ticket,
            &DispatchEvidence {
                stage: DispatchStage::Streaming,
                proof: DispatchProof::ResponseStarted,
                attempt: 1,
                provider_request_id: Some(ProviderRequestId::truncating("req-1")),
                external_operation: None,
                detached_tool: Some(DetachedOperationRef {
                    id: DetachedOperationId("op-1".to_owned()),
                    executor: ExecutorRoute::Mcp,
                }),
                receipt: Some(receipt),
                detail: None,
            },
        )
        .await;
    let body = captured(receiver);
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(
        update.contains("detachedOperationId = :detachedOperation"),
        "{update}"
    );
    assert!(
        update.contains("detachedExecutor = :detachedExecutor"),
        "{update}"
    );
    assert!(
        update.contains("providerRequestId = :providerRequestId"),
        "{update}"
    );
    assert!(update.contains("receiptHash = :receipt"), "{update}");
    let values = &body["ExpressionAttributeValues"];
    assert_eq!(values[":detachedOperation"]["S"], "op-1");
    assert_eq!(values[":detachedExecutor"]["S"], "Mcp");
    assert_eq!(values[":providerRequestId"]["S"], "req-1");
    assert_eq!(values[":receipt"]["S"], receipt.to_hex());
}

#[tokio::test]
async fn a_response_start_refuses_two_operation_authorities_before_aws() {
    use aex_brain_domain::effect::{
        DetachedOperationRef, DispatchEvidence, DispatchProof, DispatchStage,
    };
    use aex_brain_domain::ids::DetachedOperationId;
    use aex_brain_domain::journal::ExecutorRoute;

    let (store, _receiver) = capturing();
    let guard = FenceGuard::new(
        key(),
        OwnerToken(uuid::Uuid::from_u128(5)),
        Fence(7),
        AgentRevision(2),
        Some(JournalSeq(4)),
        CancelEpoch(0),
        CancelToken::new(),
    );
    let ticket = DispatchTicket::mint(
        &guard,
        authority().workspace,
        authority().organization,
        EffectId([9; 16]),
        1,
        Timestamp::from_millis(1_767_225_600_000),
    );
    let error = store
        .mark_response_started(
            &ticket,
            &DispatchEvidence {
                stage: DispatchStage::Streaming,
                proof: DispatchProof::ResponseStarted,
                attempt: 1,
                provider_request_id: None,
                external_operation: Some(DetachedOperationId("external".to_owned())),
                detached_tool: Some(DetachedOperationRef {
                    id: DetachedOperationId("tool".to_owned()),
                    executor: ExecutorRoute::Mcp,
                }),
                receipt: None,
                detail: None,
            },
        )
        .await
        .expect_err("one effect cannot be recovered through two authorities");
    assert!(format!("{error}").contains("mutually exclusive"), "{error}");
}

/// Evidence a dispatch did not produce is not written as an empty string: an absent optional
/// attribute is how the decoder reports absence, and `""` would decode as a real operation.
#[tokio::test]
async fn absent_evidence_is_left_absent_rather_than_written_empty() {
    use aex_brain_domain::effect::{DispatchEvidence, DispatchStage};

    let (store, receiver) = capturing();
    let guard = FenceGuard::new(
        key(),
        OwnerToken(uuid::Uuid::from_u128(5)),
        Fence(7),
        AgentRevision(2),
        Some(JournalSeq(4)),
        CancelEpoch(0),
        CancelToken::new(),
    );
    let ticket = DispatchTicket::mint(
        &guard,
        authority().workspace,
        authority().organization,
        EffectId([9; 16]),
        1,
        Timestamp::from_millis(1_767_225_600_000),
    );
    let _ = store
        .mark_response_started(
            &ticket,
            &DispatchEvidence::ambiguous(1, DispatchStage::Dispatched),
        )
        .await;
    let body = captured(receiver);
    let update = body["UpdateExpression"].as_str().expect("an update");
    assert!(!update.contains("OperationId"), "{update}");
    assert!(!update.contains("detachedExecutor"), "{update}");
    assert!(!update.contains("receiptHash"), "{update}");
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
            None,
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

/// A native continuation is sent back as `ExclusiveStartKey`; deriving the next query only
/// from the number of returned items would repeat or skip rows after a short service page.
#[tokio::test]
async fn a_journal_page_carries_the_complete_native_continuation() {
    let (store, receiver) = capturing();
    let partition = aex_brain_store_aws::keys::agent_partition(&key()).expect("a valid key");
    let cursor = JournalCursor::new(
        [
            (aex_session_dynamodb::attr::PK, partition.clone()),
            (
                aex_session_dynamodb::attr::SK,
                aex_brain_store_aws::keys::journal_sort_key(JournalSeq(4)),
            ),
        ],
        JournalSeq(4),
        JournalSeq(5),
    );
    let _ = store
        .read_page(
            &key(),
            JournalSeq(5),
            ReadBudget {
                max_entries: 25,
                max_bytes: 1_024,
            },
            Some(cursor),
        )
        .await;
    let body = captured(receiver);
    assert_eq!(body["ExclusiveStartKey"]["pk"]["S"], partition);
    assert_eq!(
        body["ExclusiveStartKey"]["sk"]["S"],
        "J#00000000000000000004"
    );
    assert_eq!(
        body["ExpressionAttributeValues"][":from"]["S"], "J#00000000000000000004",
        "native pagination retains the original key condition"
    );
}

/// `Limit` is not a page-length promise: `DynamoDB` stops at one service page and returns a
/// `LastEvaluatedKey` even when fewer entries than requested fit. The next read must use
/// that complete key and return each contiguous sequence exactly once.
#[tokio::test]
async fn an_early_service_page_boundary_resumes_without_skipping_or_repeating() {
    let record = finished();
    let first = entry(4, &record);
    let second = entry(5, &record);
    let last = continuation(4);
    let (store, replay) = replaying(vec![
        serde_json::json!({
            "Items": [dynamo_item(&first)],
            "Count": 1,
            "ScannedCount": 1,
            "LastEvaluatedKey": dynamo_item(&last),
        }),
        serde_json::json!({
            "Items": [dynamo_item(&second)],
            "Count": 1,
            "ScannedCount": 1,
        }),
    ]);
    let budget = budget(25, 1 << 20);

    let first_page = store
        .read_page(&key(), JournalSeq(4), budget, None)
        .await
        .expect("the short first service page decodes");
    assert_eq!(first_page.entries.len(), 1, "the Limit was 25, not one");
    let continuation = first_page.next.expect("the service supplied a LEK");
    assert_eq!(continuation.next(), JournalSeq(5));

    let second_page = store
        .read_page(&key(), JournalSeq(5), budget, Some(continuation))
        .await
        .expect("the native continuation resumes the query");
    let sequences = first_page
        .entries
        .iter()
        .chain(&second_page.entries)
        .map(|entry| entry.envelope.seq)
        .collect::<Vec<_>>();
    assert_eq!(sequences, [JournalSeq(4), JournalSeq(5)]);
    assert_eq!(second_page.next, None);

    let requests = captured_requests(&replay);
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request["ConsistentRead"], true);
        assert_eq!(request["Limit"], 25);
        assert!(request["FilterExpression"].is_null());
        assert_eq!(
            request["ExpressionAttributeValues"][":from"]["S"], "J#00000000000000000004",
            "a resumed query retains the original key condition"
        );
    }
    assert_eq!(
        requests[1]["ExclusiveStartKey"],
        dynamo_item(&last),
        "the complete service continuation is returned verbatim"
    );
}

// ---------------------------------------------------------------------------
// page rules
// ---------------------------------------------------------------------------

fn entry(seq: u64, record: &JournalRecord) -> aex_session_dynamodb::attr::Item {
    let body = record.canonical_bytes().expect("canonical");
    ItemBuilder::new(aex_session_dynamodb::codec::JOURNAL_ENTRY)
        .set(
            aex_session_dynamodb::attr::PK,
            s(aex_brain_store_aws::keys::agent_partition(&key()).expect("a valid key")),
        )
        .set(
            aex_session_dynamodb::attr::SK,
            s(aex_brain_store_aws::keys::journal_sort_key(JournalSeq(seq))),
        )
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

fn continuation(seq: u64) -> aex_session_dynamodb::attr::Item {
    aex_session_dynamodb::attr::Item::from([
        (
            aex_session_dynamodb::attr::PK.to_owned(),
            s(aex_brain_store_aws::keys::agent_partition(&key()).expect("a valid key")),
        ),
        (
            aex_session_dynamodb::attr::SK.to_owned(),
            s(aex_brain_store_aws::keys::journal_sort_key(JournalSeq(seq))),
        ),
    ])
}

fn dynamo_item(item: &aex_session_dynamodb::attr::Item) -> serde_json::Value {
    serde_json::Value::Object(
        item.iter()
            .map(|(name, value)| {
                let value = match value {
                    aws_sdk_dynamodb::types::AttributeValue::S(value) => {
                        serde_json::json!({"S": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::N(value) => {
                        serde_json::json!({"N": value})
                    }
                    aws_sdk_dynamodb::types::AttributeValue::B(value) => serde_json::json!({
                        "B": base64::prelude::BASE64_STANDARD.encode(value.as_ref())
                    }),
                    aws_sdk_dynamodb::types::AttributeValue::Bool(value) => {
                        serde_json::json!({"BOOL": value})
                    }
                    other => panic!("the journal fixture uses no {other:?} attribute"),
                };
                (name.clone(), value)
            })
            .collect(),
    )
}

fn partition() -> String {
    aex_brain_store_aws::keys::agent_partition(&key()).expect("a valid key")
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
    let page = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
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
    let error = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("a gap is a typed error, never a fold");
    assert_eq!(
        error,
        StoreError::JournalGap {
            missing: JournalSeq(5)
        }
    );
}

#[test]
fn a_page_that_is_not_in_ascending_sequence_order_is_refused() {
    let record = finished();
    let items = [entry(5, &record), entry(4, &record)];
    let error = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("query order is part of the page contract");
    assert_eq!(
        error,
        StoreError::JournalGap {
            missing: JournalSeq(4)
        }
    );
}

/// A body that does not hash to its recorded entry id is a fork. Folding either side of one
/// silently forks the agent's whole future, so the page refuses instead.
#[test]
fn a_body_that_disagrees_with_its_recorded_hash_quarantines() {
    let mut item = entry(4, &finished());
    item.insert("entryId".to_owned(), s("f".repeat(64)));
    let error = aex_brain_store_aws::journal::decode_page(
        &[item],
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
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
    let error = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1),
    )
    .expect_err("an exhausted budget is reported");
    assert!(
        matches!(error, StoreError::ReadBudgetExhausted { .. }),
        "{error:?}"
    );
}

#[test]
fn a_response_that_exceeds_the_entry_budget_is_refused_rather_than_truncated() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record)];
    let error = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(1, 1 << 20),
    )
    .expect_err("an over-limit response cannot be silently treated as EOF");
    assert!(
        matches!(error, StoreError::ReadBudgetExhausted { entries: 2, .. }),
        "{error:?}"
    );
}

#[test]
fn a_short_page_with_a_last_evaluated_key_reports_where_to_resume() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record)];
    let last_evaluated_key = continuation(5);
    let page = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&last_evaluated_key),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect("a short service page");
    assert_eq!(
        page.next.as_ref().map(JournalCursor::next),
        Some(JournalSeq(6))
    );
}

#[test]
fn a_full_page_without_a_native_continuation_is_eof() {
    let record = finished();
    let items = [entry(4, &record), entry(5, &record)];
    let page = aex_brain_store_aws::journal::decode_page(
        &items,
        None,
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(2, 1 << 20),
    )
    .expect("the service reached EOF exactly at the caller limit");
    assert_eq!(page.next, None);
}

#[test]
fn an_empty_native_continuation_is_the_service_eof_sentinel() {
    let items = [entry(4, &finished())];
    let empty = aex_session_dynamodb::attr::Item::new();
    let page = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&empty),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect("DynamoDB defines an empty LEK as the last page");
    assert_eq!(page.next, None);
}

#[test]
fn every_native_continuation_component_is_required_and_validated() {
    let items = [entry(4, &finished())];
    let mut missing_sk = continuation(4);
    missing_sk.remove(aex_session_dynamodb::attr::SK);
    let missing = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&missing_sk),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("a partial native key is unusable");
    assert!(matches!(missing, StoreError::Undecodable { .. }));

    let mut wrong_partition = continuation(4);
    wrong_partition.insert(
        aex_session_dynamodb::attr::PK.to_owned(),
        s("AGENT#another-session#another-agent"),
    );
    let wrong = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&wrong_partition),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("a continuation for another partition is unusable");
    assert!(matches!(wrong, StoreError::Undecodable { .. }));

    let wrong_tail = continuation(5);
    let wrong = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&wrong_tail),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("the LEK must identify the last decoded row");
    assert!(matches!(wrong, StoreError::Undecodable { .. }));

    let mut extra = continuation(4);
    extra.insert("anotherKey".to_owned(), s("not part of the table key"));
    let extra = aex_brain_store_aws::journal::decode_page(
        &items,
        Some(&extra),
        &partition(),
        JournalSeq(4),
        JournalSeq(4),
        budget(25, 1 << 20),
    )
    .expect_err("an expanded native key shape is not silently accepted");
    assert!(matches!(extra, StoreError::Undecodable { .. }));
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
        condition_for(Participant::WORK_WAKE_DONE),
        ConditionFailure::WakeStateMoved
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
