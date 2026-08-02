//! Bounded local load contracts for lifecycle reconciliation.

use content_lifecycle_worker::{
    ContentItem, GRACE_MILLIS, MAX_EXPIRY_WRITES_IN_FLIGHT, ReconcileOutcome, reconcile_staged,
};

#[test]
fn staged_reconciliation_remains_total_at_inventory_page_load() {
    let item = ContentItem::staged("content", "object-key", "etag", 0, 1).expect("item");
    for offset in 0..100_000 {
        let now_ms = GRACE_MILLIS - 50_000 + offset;
        let expected = if now_ms < GRACE_MILLIS {
            ReconcileOutcome::KeepStaged
        } else {
            ReconcileOutcome::ConfirmOrphan
        };
        assert_eq!(reconcile_staged(&item, now_ms), expected);
    }
}

#[test]
fn grant_expiry_has_a_fixed_write_concurrency_ceiling() {
    assert_eq!(MAX_EXPIRY_WRITES_IN_FLIGHT, 16);
}
