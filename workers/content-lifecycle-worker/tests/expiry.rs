//! Bounded expiry orchestration, failure settlement and exact-count requirements.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aex_content_dynamodb::store::{GrantExpiry, GrantExpiryPage};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{ContentHash, PrefixedId, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use async_trait::async_trait;
use content_lifecycle_worker::expiry::{ExpiryStore, expire_due_grants};
use content_lifecycle_worker::{MAX_EXPIRY_SCANS_IN_FLIGHT, MAX_EXPIRY_WRITES_IN_FLIGHT};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct Probe {
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
    started: Arc<AtomicUsize>,
    gate: Option<Arc<Semaphore>>,
}

impl Probe {
    fn open() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            maximum: Arc::new(AtomicUsize::new(0)),
            started: Arc::new(AtomicUsize::new(0)),
            gate: None,
        }
    }

    fn gated() -> Self {
        Self {
            gate: Some(Arc::new(Semaphore::new(0))),
            ..Self::open()
        }
    }

    async fn observe(&self) {
        self.started.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.acquire()
                .await
                .expect("the test gate stays open")
                .forget();
        }
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    fn release(&self, permits: usize) {
        self.gate
            .as_ref()
            .expect("a gated probe")
            .add_permits(permits);
    }
}

#[derive(Clone)]
struct FakeStore {
    scan: Probe,
    write: Probe,
    scan_failures: Arc<BTreeSet<u16>>,
    write_failures: Arc<BTreeSet<String>>,
}

impl FakeStore {
    fn new(scan: Probe, write: Probe) -> Self {
        Self {
            scan,
            write,
            scan_failures: Arc::new(BTreeSet::new()),
            write_failures: Arc::new(BTreeSet::new()),
        }
    }

    fn failing(mut self, scans: impl IntoIterator<Item = u16>, writes: &[u16]) -> Self {
        self.scan_failures = Arc::new(scans.into_iter().collect());
        self.write_failures = Arc::new(writes.iter().copied().map(token).collect());
        self
    }
}

#[async_trait]
impl ExpiryStore for FakeStore {
    async fn scan_expired_grants(
        &self,
        shard: u16,
        _now: Timestamp,
        _budget: PageBudget,
    ) -> Result<GrantExpiryPage, StoreError> {
        self.scan.observe().await;
        if self.scan_failures.contains(&shard) {
            return Err(StoreError::Unavailable {
                detail: format!("scan {shard}"),
            });
        }
        Ok(GrantExpiryPage {
            grants: vec![grant(shard)],
            more: shard.is_multiple_of(3),
        })
    }

    async fn expire_grant(&self, grant: &GrantExpiry, _now: Timestamp) -> Result<(), StoreError> {
        self.write.observe().await;
        if self.write_failures.contains(&grant.token_sha256) {
            return Err(StoreError::Unavailable {
                detail: format!("write {}", grant.token_sha256),
            });
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_shards_and_writes_run_at_their_independent_sixteen_task_bounds() {
    let scan = Probe::gated();
    let write = Probe::gated();
    let store = FakeStore::new(scan.clone(), write.clone());
    let run = tokio::spawn(expire_due_grants(store, 64, now(), budget()));

    wait_for(&scan.started, MAX_EXPIRY_SCANS_IN_FLIGHT).await;
    assert_eq!(scan.maximum.load(Ordering::SeqCst), 16);
    assert_eq!(write.started.load(Ordering::SeqCst), 0);
    scan.release(64);

    wait_for(&write.started, MAX_EXPIRY_WRITES_IN_FLIGHT).await;
    assert_eq!(write.maximum.load(Ordering::SeqCst), 16);
    write.release(64);

    let report = run
        .await
        .expect("the task joins")
        .expect("the expiry succeeds");
    assert_eq!(report.shards_attempted, 64);
    assert_eq!(report.shards_scanned, 64);
    assert_eq!(report.shards_with_more, 22);
    assert_eq!(report.grants_selected, 64);
    assert_eq!(report.expired_grants, 64);
    assert_eq!(scan.maximum.load(Ordering::SeqCst), 16);
    assert_eq!(write.maximum.load(Ordering::SeqCst), 16);
}

#[tokio::test]
async fn scan_and_write_failures_settle_every_possible_attempt_and_report_exact_counts() {
    let scan = Probe::open();
    let write = Probe::open();
    let store = FakeStore::new(scan.clone(), write.clone()).failing([1, 3], &[2]);

    let failure = expire_due_grants(store, 4, now(), budget())
        .await
        .expect_err("any settled failure fails the invocation");

    assert_eq!(scan.started.load(Ordering::SeqCst), 4);
    assert_eq!(write.started.load(Ordering::SeqCst), 2);
    assert_eq!(failure.scan_failures, 2);
    assert_eq!(failure.write_failures, 1);
    assert_eq!(failure.task_failures, 0);
    assert_eq!(failure.report.shards_attempted, 4);
    assert_eq!(failure.report.shards_scanned, 2);
    assert_eq!(failure.report.shards_with_more, 1);
    assert_eq!(failure.report.grants_selected, 2);
    assert_eq!(failure.report.expired_grants, 1);
    assert!(failure.to_string().contains("scan 1"));
}

async fn wait_for(counter: &AtomicUsize, expected: usize) {
    for _ in 0..10_000 {
        if counter.load(Ordering::SeqCst) >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!(
        "only {} of {expected} calls started",
        counter.load(Ordering::SeqCst)
    );
}

fn grant(shard: u16) -> GrantExpiry {
    GrantExpiry {
        token_sha256: token(shard),
        workspace: WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10])),
        digest: ContentHash::from_bytes([u8::try_from(shard).expect("test shard"); 32]),
        expires_at: now(),
    }
}

fn token(shard: u16) -> String {
    format!("{shard:064x}")
}

fn now() -> Timestamp {
    Timestamp::parse("2026-08-02T12:34:56.789Z").expect("the pinned instant")
}

fn budget() -> PageBudget {
    PageBudget::new(100).expect("the admitted maximum")
}
