//! The SQS usage-ingress producer against a real queue, on `moto`.
//!
//! What this proves. The drafts the worker actually derives — not hand-built
//! stand-ins — survive the producer/consumer wire end to end: `derive_facts`
//! produces the compute and storage drafts one closed lifecycle interval owes,
//! [`SqsFactDraftSink`] encodes each one, a real queue accepts and returns the
//! body byte for byte, and the strict `deny_unknown_fields` envelope decode on
//! the consumer side yields back exactly the draft that was emitted. It also
//! proves the two category fences are real: a sink bound to one category writes
//! only to its own queue, and a sibling category is refused *before* anything
//! reaches the wire, which the queue itself is the witness for. Finally it
//! proves the send path classifies a service refusal as a typed
//! [`SinkError`] rather than panicking or losing the draft.
//!
//! What it cannot prove, stated rather than assumed.
//!
//! - `moto` is an emulator. It does not reproduce SQS's at-least-once
//!   redelivery, its visibility-timeout timing, or its retention window, so
//!   nothing here is evidence about redrive behaviour.
//! - Standard queues promise no order, so every assertion below is set-based.
//!   `moto` happens to return messages in insertion order; real SQS does not,
//!   and a test that leaned on that would pass for the wrong reason.
//! - There is no FIFO shape to prove. The adapter sends no `MessageGroupId` and
//!   no `MessageDeduplicationId`, because both usage ingresses are standard
//!   queues. If either ingress ever became a `.fifo` queue, `SendMessage` would
//!   fail at runtime and nothing here would catch it — that is a live concern.
//! - `moto` enforces no queue policy, so scoping `sqs:SendMessage` to exactly
//!   one queue per worker role stays a live concern too.
//! - The 256 KiB body ceiling the adapter checks locally is unit evidence. No
//!   draft this crate derives comes near it, so no engine proves that branch.
//! - The last case proves the adapter maps *a* refusal onto
//!   `SinkError::Unavailable`. It cannot prove which real AWS failures are
//!   genuinely retryable; see the note on that test.

use aex_hands_protocol::lifecycle::RuntimeReceipt;
use aex_internal_contracts::PricingVersion;
use aex_runtime_control::lifecycle::{LifecycleIntentId, MicrovmId, snapshot_lifecycle_id};
use aex_runtime_control::usage::{
    FactContext, SinkError, SnapshotIo, SnapshotResidence, UsageFactSink as _, derive_facts,
};
use aex_runtime_control_aws::usage_ingress::SqsFactDraftSink;
use aex_test_harness::MotoContainer;
use aex_usage_domain::fact::FactDraft;
use aex_usage_domain::ingress::FactDraftEnvelope;
use aex_usage_domain::meter::Category;
use aex_wire::ids::{GenerationId, OrganizationId, PrefixedId as _, SessionId, Uuid7, WorkspaceId};
use aex_wire::types::{ComputeSize, Region, Timestamp};
use aws_sdk_sqs::Client;
use aws_sdk_sqs::config::{BehaviorVersion, Credentials, Region as SigningRegion};

/// How many bounded long-polls one drain may take before it gives up.
const RECEIVE_ATTEMPTS: u32 = 10;

/// How long each receive waits *server-side*. Nothing in this file sleeps.
const RECEIVE_WAIT_SECONDS: i32 = 1;

/// The running half of the closed interval the drafts are derived from.
const RUNNING_MS: u64 = 120_000;

/// The suspended half, which is also the snapshot residence.
const SUSPENDED_MS: u64 = 180_000;

fn client(engine: &MotoContainer) -> Client {
    let config = aws_sdk_sqs::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(SigningRegion::new(engine.region()))
        .endpoint_url(engine.endpoint_url())
        .credentials_provider(Credentials::new(
            engine.access_key_id(),
            engine.secret_access_key(),
            None,
            None,
            "aex-integration",
        ))
        .build();
    Client::from_conf(config)
}

async fn queue(client: &Client, name: &str) -> String {
    client
        .create_queue()
        .queue_name(name)
        .send()
        .await
        .expect("the queue is created")
        .queue_url
        .expect("a created queue has a URL")
}

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis).expect("a bounded instant")
}

/// The receipt and residence one ordinary suspend-then-terminate leaves behind.
///
/// The interval is exact — `running_ms + suspended_ms` exhausts it — because
/// `derive_facts` refuses anything else.
fn receipt() -> RuntimeReceipt {
    RuntimeReceipt {
        generation: GenerationId::from_uuid7(Uuid7::compose(11, [1; 10])),
        shape: ComputeSize::Gb1,
        running_ms: RUNNING_MS,
        suspended_ms: SUSPENDED_MS,
        from: at(0),
        to: at(i64::try_from(RUNNING_MS + SUSPENDED_MS).expect("a bounded interval")),
        snapshot_bytes: None,
        transmit_bytes: None,
    }
}

fn residence() -> SnapshotResidence {
    SnapshotResidence {
        lifecycle_id: snapshot_lifecycle_id(&MicrovmId("mvm-integration-1".to_owned()), 0),
        generation: 0,
        bytes: 5_000_000,
        suspended_at: at(i64::try_from(RUNNING_MS).expect("a bounded instant")),
        released_at: at(i64::try_from(RUNNING_MS + SUSPENDED_MS).expect("a bounded instant")),
        terminal: false,
        io: SnapshotIo::default(),
    }
}

fn context() -> FactContext {
    FactContext {
        organization: OrganizationId::from_uuid7(Uuid7::compose(2, [2; 10])),
        workspace: WorkspaceId::from_uuid7(Uuid7::compose(3, [3; 10])),
        region: Region::EuWest1,
        session: SessionId::from_uuid7(Uuid7::compose(4, [4; 10])),
        pricing_version: PricingVersion("2026-08-01".to_owned()),
        intent: LifecycleIntentId("lci-integration-1".to_owned()),
        source_receipt_id: "lambda-microvm:mvm-integration-1:suspend:req-1".to_owned(),
    }
}

/// Exactly the drafts the worker emits for one closed interval with a snapshot:
/// compute millicpu, compute memory, and storage byte-minutes.
fn drafts() -> Vec<FactDraft> {
    derive_facts(&receipt(), Some(&residence()), &context()).expect("the interval is exact")
}

/// Long-polls until `expected` bodies have arrived or the attempt budget runs
/// out. The wait is server-side, so this never sleeps and never asserts on a
/// timer.
async fn drain(client: &Client, queue_url: &str, expected: usize, attempts: u32) -> Vec<String> {
    let mut bodies = Vec::new();
    for _ in 0..attempts {
        if bodies.len() >= expected {
            break;
        }
        let batch = client
            .receive_message()
            .queue_url(queue_url)
            .max_number_of_messages(10)
            .wait_time_seconds(RECEIVE_WAIT_SECONDS)
            .send()
            .await
            .expect("the queue is readable");
        for message in batch.messages() {
            bodies.push(message.body().expect("a message carries a body").to_owned());
        }
    }
    bodies.sort();
    bodies
}

/// What the adapter must have put on the wire, derived independently of it.
fn encoded(category: Category, draft: FactDraft) -> String {
    serde_json::to_string(&FactDraftEnvelope::new(category, draft).expect("the fences agree"))
        .expect("the envelope serializes")
}

#[tokio::test(flavor = "multi_thread")]
async fn every_draft_one_closed_interval_owes_round_trips_through_its_own_category_queue() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let compute_url = queue(&client, "aex-integration-usage-compute").await;
    let storage_url = queue(&client, "aex-integration-usage-storage").await;

    let compute = SqsFactDraftSink::new(client.clone(), &compute_url, Category::Compute);
    let storage = SqsFactDraftSink::new(client.clone(), &storage_url, Category::Storage);
    assert_eq!(compute.queue_url(), compute_url);

    let derived = drafts();
    let mut expected_compute = Vec::new();
    let mut expected_storage = Vec::new();
    for draft in derived {
        let category = draft.authority.category;
        let sink = match category {
            Category::Compute => &compute,
            Category::Storage => &storage,
            Category::Transfer => panic!("this worker holds no transfer binding"),
        };
        let wire = encoded(category, draft.clone());
        sink.emit(category, draft).await.expect("the draft is sent");
        match category {
            Category::Compute => expected_compute.push(wire),
            Category::Storage => expected_storage.push(wire),
            Category::Transfer => unreachable!("refused above"),
        }
    }
    expected_compute.sort();
    expected_storage.sort();
    assert_eq!(
        expected_compute.len(),
        2,
        "one closed interval owes compute millicpu and compute memory"
    );
    assert_eq!(expected_storage.len(), 1, "and one storage residence");

    let received_compute = drain(&client, &compute_url, 2, RECEIVE_ATTEMPTS).await;
    let received_storage = drain(&client, &storage_url, 1, RECEIVE_ATTEMPTS).await;
    assert_eq!(
        received_compute, expected_compute,
        "the queue returned a body the adapter did not encode"
    );
    assert_eq!(received_storage, expected_storage);

    // The consumer end of the contract: strict decode, then the worker-side
    // category fence, then the draft itself.
    for body in received_compute {
        let envelope: FactDraftEnvelope =
            serde_json::from_str(&body).expect("the body the queue returned decodes strictly");
        assert_eq!(envelope.category, Category::Compute);
        let draft = envelope
            .into_draft(Category::Compute)
            .expect("a compute envelope reaches the compute worker");
        assert_eq!(draft.authority.category, Category::Compute);
        assert_eq!(draft.service.as_str(), "runtime-control-worker");
    }
    let storage_draft: FactDraftEnvelope =
        serde_json::from_str(&received_storage[0]).expect("the storage body decodes strictly");
    assert_eq!(
        storage_draft
            .into_draft(Category::Storage)
            .expect("a storage envelope reaches the storage worker"),
        drafts()
            .into_iter()
            .find(|draft| draft.authority.category == Category::Storage)
            .expect("a storage draft"),
        "the draft came back off a real queue exactly as it went on"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sibling_categorys_draft_is_refused_before_anything_reaches_the_queue() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let compute_url = queue(&client, "aex-integration-usage-compute-fence").await;
    let sink = SqsFactDraftSink::new(client.clone(), &compute_url, Category::Compute);

    let storage_draft = drafts()
        .into_iter()
        .find(|draft| draft.authority.category == Category::Storage)
        .expect("a storage draft");

    // The call addresses the sibling authority: the sink's own binding refuses.
    let addressed = sink
        .emit(Category::Storage, storage_draft.clone())
        .await
        .expect_err("a storage call on a compute-bound sink");
    assert!(
        matches!(addressed, SinkError::Refused { .. }),
        "{addressed}"
    );

    // The call addresses the right authority but carries a draft keyed to the
    // other one: the envelope's redundant fence refuses.
    let smuggled = sink
        .emit(Category::Compute, storage_draft)
        .await
        .expect_err("a storage draft inside a compute envelope");
    assert!(matches!(smuggled, SinkError::Refused { .. }), "{smuggled}");

    assert!(
        drain(&client, &compute_url, 1, 1).await.is_empty(),
        "a refused draft must never reach the queue; the fence is not server-side"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queue_the_service_does_not_hold_is_a_typed_sink_error_and_never_a_panic() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    // A well-formed URL on a live endpoint naming a queue nobody created: the
    // shape a mistyped `AEX_USAGE_COMPUTE_QUEUE_URL` takes.
    let absent = format!(
        "{}/123456789012/aex-integration-usage-compute-never-created",
        engine.endpoint_url()
    );
    let compute = drafts()
        .into_iter()
        .find(|draft| draft.authority.category == Category::Compute)
        .expect("a compute draft");

    let error = SqsFactDraftSink::new(client, &absent, Category::Compute)
        .emit(Category::Compute, compute)
        .await
        .expect_err("a queue the service does not hold");

    // NOTE: `Unavailable` is what `flush_usage` maps onto `CommandOutcome::Retry`,
    // so a permanently misconfigured queue URL is redriven exactly as a transient
    // outage is, up to `MAX_RECEIVE_COUNT`, and only then quarantined. That
    // conflation is pinned here deliberately rather than hidden; it is reported
    // upstream, not fixed in a test.
    assert!(
        matches!(
            error,
            SinkError::Unavailable {
                category: Category::Compute,
                ..
            }
        ),
        "{error}"
    );
}
