//! The SQS usage-ingress producer against a real queue, on `moto`.
//!
//! What this proves. The drafts the worker actually derives — not hand-built
//! stand-ins — survive the producer/consumer wire end to end: `derive_facts`
//! produces the compute and storage drafts one closed lifecycle interval owes,
//! [`SqsFactDraftSink`] validates and converts each one to the central rating
//! contract, a real FIFO accepts and returns every body, and the strict
//! `deny_unknown_fields` consumer type decodes it. It also proves the two
//! category fences are real although both sinks share one organization-ordered
//! queue: a sibling category is refused *before* anything reaches the wire.
//! Finally it
//! proves the send path classifies a service refusal as a typed
//! [`SinkError`] rather than panicking or losing the draft.
//!
//! What it cannot prove, stated rather than assumed.
//!
//! - `moto` is an emulator. It does not reproduce SQS's at-least-once
//!   redelivery, its visibility-timeout timing, or its retention window, so
//!   nothing here is evidence about redrive behaviour.
//! - The queue is FIFO and every send carries producer-derived group and dedupe
//!   identities. `moto` does not prove real SQS redelivery timing, so assertions
//!   remain set-based.
//! - `moto` enforces no queue policy, so scoping `sqs:SendMessage` to exactly
//!   one queue per worker role stays a live concern too.
//! - The 256 KiB body ceiling the adapter checks locally is unit evidence. No
//!   draft this crate derives comes near it, so no engine proves that branch.
//! - The last case proves the adapter maps *a* refusal onto
//!   `SinkError::Unavailable`. It cannot prove which real AWS failures are
//!   genuinely retryable; see the note on that test.

use aex_hands_protocol::lifecycle::RuntimeReceipt;
use aex_internal_contracts::PricingVersion;
use aex_internal_contracts::usage::RatingRequest;
use aex_runtime_control::lifecycle::{LifecycleIntentId, MicrovmId, snapshot_lifecycle_id};
use aex_runtime_control::usage::{
    FactContext, SinkError, SnapshotIo, SnapshotResidence, UsageFactSink as _, derive_facts,
};
use aex_runtime_control_aws::usage_ingress::SqsFactDraftSink;
use aex_test_harness::MotoContainer;
use aex_usage_domain::fact::FactDraft;
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
        .queue_name(format!("{name}.fifo"))
        .attributes(aws_sdk_sqs::types::QueueAttributeName::FifoQueue, "true")
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

#[tokio::test(flavor = "multi_thread")]
async fn every_draft_one_closed_interval_owes_reaches_the_shared_billing_fifo() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let rating_url = queue(&client, "aex-integration-usage-rating").await;

    let compute = SqsFactDraftSink::new(client.clone(), &rating_url, Category::Compute);
    let storage = SqsFactDraftSink::new(client.clone(), &rating_url, Category::Storage);
    assert_eq!(compute.queue_url(), rating_url);

    let derived = drafts();
    let expected_ids = derived
        .iter()
        .map(FactDraft::fact_id)
        .map(|id| id.to_string().trim_start_matches("usage_").to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    for draft in derived {
        let category = draft.authority.category;
        let sink = match category {
            Category::Compute => &compute,
            Category::Storage => &storage,
            Category::Transfer => panic!("this worker holds no transfer binding"),
        };
        sink.emit(category, draft).await.expect("the draft is sent");
    }

    let received = drain(&client, &rating_url, 3, RECEIVE_ATTEMPTS).await;
    assert_eq!(received.len(), 3);
    let decoded = received
        .iter()
        .map(|body| serde_json::from_str::<RatingRequest>(body).expect("strict rating request"))
        .collect::<Vec<_>>();
    assert_eq!(
        decoded
            .iter()
            .map(|request| request.fact.fact_id.to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        expected_ids
    );
    assert_eq!(
        decoded
            .iter()
            .filter(|request| request.fact.meter.category() == "compute")
            .count(),
        2
    );
    assert_eq!(
        decoded
            .iter()
            .filter(|request| request.fact.meter.category() == "storage")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sibling_categorys_draft_is_refused_before_anything_reaches_the_queue() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    let rating_url = queue(&client, "aex-integration-usage-rating-fence").await;
    let sink = SqsFactDraftSink::new(client.clone(), &rating_url, Category::Compute);

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
        drain(&client, &rating_url, 1, 1).await.is_empty(),
        "a refused draft must never reach the queue; the fence is not server-side"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queue_the_service_does_not_hold_is_a_typed_sink_error_and_never_a_panic() {
    let engine = MotoContainer::start().await.expect("moto starts");
    let client = client(&engine);
    // A well-formed URL on a live endpoint naming a queue nobody created: the
    // shape a mistyped `AEX_USAGE_RATING_QUEUE_URL` takes.
    let absent = format!(
        "{}/123456789012/aex-integration-usage-rating-never-created.fifo",
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
