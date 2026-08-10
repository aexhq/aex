//! The delete batch actually deletes, and only genuine retries are reported.
//!
//! The defect these pin: `Mode::Delete` used to report **every** SQS record as a
//! batch-item failure without ever calling S3, so the delete queue drained to its
//! dead-letter queue and no object was ever reclaimed — while the worker returned
//! a well-formed response the whole time.

use std::sync::Mutex;

use aex_content_dynamodb::codec::{ContentDescriptor, GcEpoch, TreePage};
use aex_content_dynamodb::store::{
    ContentMetadataStore, GcScanPage, GrantExpiry, GrantExpiryPage, Reachability, RedeemedGrant,
};
use aex_content_dynamodb::wire_pending::{GcSweepPlan, InlineBody};
use aex_session_dynamodb::error::StoreError;
use aex_session_dynamodb::paging::PageBudget;
use aex_wire::ids::{ContentHash, OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
use aex_wire::types::Timestamp;
use content_lifecycle_worker::gc::{DeletableObjects, run_delete_batch};
use content_lifecycle_worker::{DeleteIntent, ObjectDeleteResult};

const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

fn workspace_id() -> String {
    WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10])).to_string()
}

fn organization_id() -> String {
    OrganizationId::from_uuid7(Uuid7::compose(1_754_051_696_789, [2; 10])).to_string()
}

fn now() -> Timestamp {
    // Well past the 24-hour grace of a body staged at the epoch.
    Timestamp::from_unix_millis(48 * 60 * 60 * 1_000).expect("in range")
}

fn body(fence: u64) -> String {
    serde_json::json!({
        "workspaceId": workspace_id(),
        "organizationId": organization_id(),
        "digest": DIGEST,
        "objectKey": "wks/00/00/0000",
        "objectEtag": "\"marked\"",
        "markedEpoch": 3,
        "epoch": 4,
        "stagedAtMs": 0,
        "fence": fence
    })
    .to_string()
}

/// A store that reports one reachability answer and records every sweep.
struct Store {
    reachability: Reachability,
    swept: Mutex<Vec<GcSweepPlan>>,
}

impl Store {
    fn collectable() -> Self {
        Self {
            reachability: Reachability {
                pins: 0,
                unexpired_grants: 0,
            },
            swept: Mutex::new(Vec::new()),
        }
    }

    fn pinned() -> Self {
        Self {
            reachability: Reachability {
                pins: 1,
                unexpired_grants: 0,
            },
            swept: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl ContentMetadataStore for Store {
    async fn load_descriptor(
        &self,
        _workspace: WorkspaceId,
        _digest: &ContentHash,
    ) -> Result<Option<ContentDescriptor>, StoreError> {
        Ok(None)
    }

    async fn read_inline_body(
        &self,
        _workspace: WorkspaceId,
        _digest: &ContentHash,
    ) -> Result<Option<InlineBody>, StoreError> {
        Ok(None)
    }

    async fn read_tree_page(
        &self,
        _workspace: WorkspaceId,
        _page: aex_content_dynamodb::wire_pending::Blake3Digest,
    ) -> Result<Option<TreePage>, StoreError> {
        Ok(None)
    }

    async fn put_tree_pages(&self, _pages: &[TreePage]) -> Result<(), StoreError> {
        Ok(())
    }

    async fn load_gc_epoch(&self, _workspace: WorkspaceId) -> Result<Option<GcEpoch>, StoreError> {
        Ok(None)
    }

    async fn scan_gc_bucket(
        &self,
        _workspace: WorkspaceId,
        _bucket: u16,
        _budget: PageBudget,
    ) -> Result<GcScanPage, StoreError> {
        Ok(GcScanPage {
            entries: Vec::new(),
            more: false,
        })
    }

    async fn reachability(
        &self,
        _workspace: WorkspaceId,
        _digest: &ContentHash,
        _now: Timestamp,
    ) -> Result<Reachability, StoreError> {
        Ok(self.reachability.clone())
    }

    async fn sweep_candidate(&self, plan: &GcSweepPlan) -> Result<(), StoreError> {
        self.swept.lock().expect("not poisoned").push(plan.clone());
        Ok(())
    }

    async fn mint_grant(
        &self,
        _grant: &aex_content_dynamodb::codec::DownloadGrant,
        _now: Timestamp,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn scan_expired_grants(
        &self,
        _shard: u16,
        _now: Timestamp,
        _budget: PageBudget,
        _after: Option<&aex_content_dynamodb::codec::GrantExpiryPosition>,
    ) -> Result<GrantExpiryPage, StoreError> {
        Ok(GrantExpiryPage {
            grants: Vec::new(),
            next: None,
        })
    }

    async fn load_grant_expiry_cursor(
        &self,
        _shard: u16,
    ) -> Result<Option<aex_content_dynamodb::codec::GrantExpiryCursor>, StoreError> {
        Ok(None)
    }

    async fn advance_grant_expiry_cursor(
        &self,
        _current: Option<&aex_content_dynamodb::codec::GrantExpiryCursor>,
        _shard: u16,
        _position: Option<&aex_content_dynamodb::codec::GrantExpiryPosition>,
        _now: Timestamp,
    ) -> Result<(), StoreError> {
        Ok(())
    }

    async fn expire_grant(&self, _grant: &GrantExpiry, _now: Timestamp) -> Result<(), StoreError> {
        Ok(())
    }

    async fn redeem_grant(
        &self,
        _token_sha256_hex: &str,
        _now: Timestamp,
    ) -> Result<RedeemedGrant, StoreError> {
        Err(StoreError::Invalid {
            detail: "the delete role never redeems a grant".to_owned(),
        })
    }
}

/// An object store that answers one result and records every request.
struct Objects {
    result: ObjectDeleteResult,
    requests: Mutex<Vec<DeleteIntent>>,
}

impl Objects {
    fn answering(result: ObjectDeleteResult) -> Self {
        Self {
            result,
            requests: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl DeletableObjects for Objects {
    async fn delete_fenced(&self, intent: &DeleteIntent) -> Result<ObjectDeleteResult, String> {
        self.requests
            .lock()
            .expect("not poisoned")
            .push(intent.clone());
        Ok(self.result)
    }
}

#[tokio::test]
async fn a_collectable_record_is_deleted_under_its_etag_and_acknowledged() {
    let store = Store::collectable();
    let objects = Objects::answering(ObjectDeleteResult::Deleted);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[("m-1".to_owned(), body(7))],
        now(),
    )
    .await;

    assert_eq!(report.deleted, 1);
    assert!(
        report.failed.is_empty(),
        "a record whose object is gone must be acknowledged, not retried"
    );
    let requests = objects.requests.lock().expect("not poisoned");
    assert_eq!(requests.len(), 1, "the batch actually reached S3");
    assert_eq!(requests[0].key, "wks/00/00/0000");
    assert_eq!(
        requests[0].if_match, "\"marked\"",
        "every delete is fenced on the ETag the sweep recorded"
    );
    assert_eq!(store.swept.lock().expect("not poisoned").len(), 1);
}

#[tokio::test]
async fn a_body_that_regained_a_pin_is_never_deleted() {
    let store = Store::pinned();
    let objects = Objects::answering(ObjectDeleteResult::Deleted);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[("m-1".to_owned(), body(7))],
        now(),
    )
    .await;

    assert_eq!(report.kept, 1);
    assert!(
        objects.requests.lock().expect("not poisoned").is_empty(),
        "the reachability recheck is taken now, not trusted from the message"
    );
    assert!(store.swept.lock().expect("not poisoned").is_empty());
}

#[tokio::test]
async fn an_object_that_changed_since_it_was_marked_keeps_its_body() {
    let store = Store::collectable();
    let objects = Objects::answering(ObjectDeleteResult::PreconditionFailed);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[("m-1".to_owned(), body(7))],
        now(),
    )
    .await;

    assert_eq!(report.kept, 1);
    assert!(report.failed.is_empty());
    assert!(
        store.swept.lock().expect("not poisoned").is_empty(),
        "a body that survived is not terminalised as deleted"
    );
}

#[tokio::test]
async fn an_already_absent_object_is_an_idempotent_success() {
    let store = Store::collectable();
    let objects = Objects::answering(ObjectDeleteResult::Missing);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[("m-1".to_owned(), body(7))],
        now(),
    )
    .await;

    assert_eq!(report.already_absent, 1);
    assert!(report.failed.is_empty());
    assert_eq!(store.swept.lock().expect("not poisoned").len(), 1);
}

#[tokio::test]
async fn only_the_records_that_must_be_retried_are_reported_as_failures() {
    let store = Store::collectable();
    let objects = Objects::answering(ObjectDeleteResult::Retryable);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[
            ("m-1".to_owned(), body(7)),
            ("m-2".to_owned(), "not json at all".to_owned()),
        ],
        now(),
    )
    .await;

    assert_eq!(report.failed, vec!["m-1".to_owned(), "m-2".to_owned()]);
    assert_eq!(report.deleted, 0);
}

#[tokio::test]
async fn a_malformed_record_never_reaches_the_object_store() {
    let store = Store::collectable();
    let objects = Objects::answering(ObjectDeleteResult::Deleted);
    let report = run_delete_batch(
        &store,
        &objects,
        "000000000000",
        &[("m-1".to_owned(), "{}".to_owned())],
        now(),
    )
    .await;

    assert_eq!(report.failed, vec!["m-1".to_owned()]);
    assert!(objects.requests.lock().expect("not poisoned").is_empty());
}
