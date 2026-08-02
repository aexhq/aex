//! Durable expiry fairness, concurrency and settled-failure requirements.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use aex_content_dynamodb::codec::{GrantExpiryCursor, GrantExpiryPosition};
use aex_content_dynamodb::keys;
use aex_content_dynamodb::store::{GrantExpiry, GrantExpiryPage};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_session_dynamodb::plan::Participant;
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
    reads: Probe,
    writes: Probe,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    rows: HashMap<u16, Vec<RowState>>,
    cursors: HashMap<u16, GrantExpiryCursor>,
    scan_failures: BTreeSet<u16>,
    write_failures: BTreeSet<String>,
    scan_starts: Vec<(u16, Option<GrantExpiryPosition>)>,
    advance_calls: usize,
}

struct RowState {
    grant: GrantExpiry,
    position: GrantExpiryPosition,
    alive: bool,
}

impl FakeStore {
    fn one_per_shard(shards: u16, reads: Probe, writes: Probe) -> Self {
        let mut state = State::default();
        for shard in 0..shards {
            state.rows.insert(shard, vec![row(shard, 0)]);
        }
        Self {
            reads,
            writes,
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn fairness() -> Self {
        let mut state = State::default();
        let rows = vec![row(0, 0), row(0, 1), row(0, 2)];
        state
            .write_failures
            .extend(rows[..2].iter().map(|row| row.grant.token_sha256.clone()));
        state.rows.insert(0, rows);
        Self {
            reads: Probe::open(),
            writes: Probe::open(),
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn fail_scan(self, shard: u16) -> Self {
        self.state
            .lock()
            .expect("state lock")
            .scan_failures
            .insert(shard);
        self
    }
}

#[async_trait]
impl ExpiryStore for FakeStore {
    async fn load_cursor(&self, shard: u16) -> Result<Option<GrantExpiryCursor>, StoreError> {
        self.reads.observe().await;
        Ok(self
            .state
            .lock()
            .expect("state lock")
            .cursors
            .get(&shard)
            .cloned())
    }

    async fn scan_expired_grants(
        &self,
        shard: u16,
        _now: Timestamp,
        budget: PageBudget,
        after: Option<&GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError> {
        self.reads.observe().await;
        let mut state = self.state.lock().expect("state lock");
        state.scan_starts.push((shard, after.cloned()));
        if state.scan_failures.contains(&shard) {
            return Err(StoreError::Unavailable {
                detail: format!("scan {shard}"),
            });
        }
        let rows = state.rows.get(&shard).expect("fixture shard");
        let start = match after {
            None => 0,
            Some(after) => rows
                .iter()
                .position(|row| &row.position == after)
                .map(|index| index + 1)
                .ok_or(StoreError::Invalid {
                    detail: "unknown fake provider key".to_owned(),
                })?,
        };
        let admitted = usize::try_from(budget.limit()).expect("page budget fits usize");
        let selected: Vec<_> = rows
            .iter()
            .skip(start)
            .filter(|row| row.alive)
            .take(admitted)
            .collect();
        let last_index = selected
            .last()
            .and_then(|last| rows.iter().position(|row| row.position == last.position));
        let has_more =
            last_index.is_some_and(|last| rows.iter().skip(last + 1).any(|row| row.alive));
        Ok(GrantExpiryPage {
            grants: selected.iter().map(|row| row.grant.clone()).collect(),
            next: has_more.then(|| selected.last().expect("a last row").position.clone()),
        })
    }

    async fn expire_grant(&self, grant: &GrantExpiry, _now: Timestamp) -> Result<(), StoreError> {
        self.writes.observe().await;
        let mut state = self.state.lock().expect("state lock");
        if state.write_failures.contains(&grant.token_sha256) {
            return Err(StoreError::Unavailable {
                detail: format!("write {}", grant.token_sha256),
            });
        }
        for rows in state.rows.values_mut() {
            if let Some(row) = rows
                .iter_mut()
                .find(|row| row.grant.token_sha256 == grant.token_sha256)
            {
                row.alive = false;
                return Ok(());
            }
        }
        Ok(())
    }

    async fn advance_cursor(
        &self,
        current: Option<&GrantExpiryCursor>,
        shard: u16,
        position: Option<&GrantExpiryPosition>,
        now: Timestamp,
    ) -> Result<(), StoreError> {
        self.writes.observe().await;
        let mut state = self.state.lock().expect("state lock");
        state.advance_calls += 1;
        let durable = state.cursors.get(&shard);
        if durable != current {
            let revision = current.map_or(1, |cursor| cursor.revision + 1);
            if durable.is_some_and(|cursor| {
                cursor.revision == revision && cursor.position.as_ref() == position
            }) {
                return Ok(());
            }
            return Err(StoreError::PreconditionFailed {
                participant: Participant::CONTENT_GRANT_EXPIRY_CURSOR,
                observed: None,
            });
        }
        state.cursors.insert(
            shard,
            GrantExpiryCursor {
                shard,
                position: position.cloned(),
                revision: current.map_or(1, |cursor| cursor.revision + 1),
                updated_at: now,
            },
        );
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_provider_reads_and_writes_obey_their_global_sixteen_operation_bounds() {
    let reads = Probe::gated();
    let writes = Probe::gated();
    let store = FakeStore::one_per_shard(64, reads.clone(), writes.clone());
    let run = tokio::spawn(expire_due_grants(store, 64, now(), budget(100)));

    wait_for(&reads.started, MAX_EXPIRY_SCANS_IN_FLIGHT).await;
    wait_for(&reads.maximum, MAX_EXPIRY_SCANS_IN_FLIGHT).await;
    assert_eq!(reads.maximum.load(Ordering::SeqCst), 16);
    assert_eq!(writes.started.load(Ordering::SeqCst), 0);
    reads.release(128);

    wait_for(&writes.started, MAX_EXPIRY_WRITES_IN_FLIGHT).await;
    wait_for(&writes.maximum, MAX_EXPIRY_WRITES_IN_FLIGHT).await;
    assert_eq!(writes.maximum.load(Ordering::SeqCst), 16);
    writes.release(128);

    let report = run.await.expect("the task joins").expect("expiry succeeds");
    assert_eq!(report.shards_attempted, 64);
    assert_eq!(report.shards_scanned, 64);
    assert_eq!(report.grants_selected, 64);
    assert_eq!(report.expired_grants, 64);
    assert_eq!(report.cursors_advanced, 64);
    assert_eq!(report.cursors_wrapped, 64);
    assert_eq!(reads.maximum.load(Ordering::SeqCst), 16);
    assert_eq!(writes.maximum.load(Ordering::SeqCst), 16);
}

#[tokio::test]
async fn a_query_failure_never_writes_that_shards_cursor() {
    let store = FakeStore::one_per_shard(1, Probe::open(), Probe::open()).fail_scan(0);
    let failure = expire_due_grants(store.clone(), 1, now(), budget(2))
        .await
        .expect_err("the query failure fails loud");
    assert_eq!(failure.scan_failures, 1);
    assert_eq!(failure.report.shards_scanned, 0);
    assert_eq!(failure.report.cursors_advanced, 0);
    assert_eq!(store.state.lock().expect("state lock").advance_calls, 0);
}

#[tokio::test]
async fn a_permanently_failed_full_page_advances_then_wraps_and_is_revisited() {
    let store = FakeStore::fairness();

    let first = expire_due_grants(store.clone(), 1, now(), budget(2))
        .await
        .expect_err("both rows fail, after all writes settle");
    assert_eq!(first.write_failures, 2);
    assert_eq!(first.report.grants_selected, 2);
    assert_eq!(first.report.cursors_advanced, 1);
    assert_eq!(first.report.cursors_wrapped, 0);
    let first_position = store
        .state
        .lock()
        .expect("state lock")
        .cursors
        .get(&0)
        .expect("durable cursor")
        .position
        .clone()
        .expect("the first page has another row");

    let second = expire_due_grants(store.clone(), 1, now(), budget(2))
        .await
        .expect("the later row succeeds");
    assert_eq!(second.grants_selected, 1);
    assert_eq!(second.expired_grants, 1);
    assert_eq!(second.cursors_wrapped, 1);

    let third = expire_due_grants(store.clone(), 1, now(), budget(2))
        .await
        .expect_err("the wrapped pass revisits both permanent failures");
    assert_eq!(third.report.grants_selected, 2);
    let state = store.state.lock().expect("state lock");
    assert_eq!(state.scan_starts[1].1.as_ref(), Some(&first_position));
    assert!(state.scan_starts[2].1.is_none());
}

async fn wait_for(counter: &AtomicUsize, expected: usize) {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if counter.load(Ordering::SeqCst) >= expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} of {expected} calls started",
            counter.load(Ordering::SeqCst)
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
}

fn row(shard: u16, ordinal: u64) -> RowState {
    let token = token_for_shard(shard, ordinal);
    let grant_key = keys::grant(&token).expect("token enters a key");
    let expires_at = now();
    RowState {
        grant: GrantExpiry {
            token_sha256: token.clone(),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10])),
            digest: ContentHash::from_bytes([u8::try_from(shard).expect("test shard"); 32]),
            expires_at,
        },
        position: GrantExpiryPosition {
            expiry_partition: keys::expiry_partition(shard),
            expiry_sort: keys::expiry_sort(expires_at, &token).expect("token enters a key"),
            grant_pk: grant_key.pk,
            grant_sk: grant_key.sk,
        },
        alive: true,
    }
}

fn token_for_shard(shard: u16, ordinal: u64) -> String {
    let mut candidate = 0_u64;
    let mut remaining = ordinal;
    loop {
        let token = format!("{candidate:064x}");
        if keys::expiry_shard(&token) == shard {
            if remaining == 0 {
                return token;
            }
            remaining -= 1;
        }
        candidate += 1;
    }
}

fn now() -> Timestamp {
    Timestamp::parse("2026-08-02T12:34:56.789Z").expect("the pinned instant")
}

fn budget(items: u32) -> PageBudget {
    PageBudget::new(items).expect("the admitted page")
}
