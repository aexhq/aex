//! The `central-control-worker` deployable against a real plane.
//!
//! Every case here needs a deployed reconciliation plane and credentials. None of it
//! can be earned in this run (OD-07), so each test **fails loudly** when the
//! live lane selects it rather than self-skipping: a green suite that ran
//! nothing is worse than a red one that says why.
//!
//! The halves that need no plane already pass in the default lane. What is
//! unavailable here is the part that needs the platform to actually do
//! something.

/// The message every unavailable live case fails with.
const UNAVAILABLE: &str =
    "live evidence requires a deployed plane and credentials; this run deploys nothing (OD-07)";

#[test]
fn a_duplicate_async_wake_reconciles_rather_than_dispatching_twice() {
    panic!("{UNAVAILABLE}: redeliver a settled transaction wake against a live region");
}

#[test]
fn a_precommit_wake_is_retried_after_the_transaction_commits() {
    panic!("{UNAVAILABLE}: hold the producer transaction open through the first async delivery");
}

#[test]
fn an_exhausted_async_failure_lands_in_the_unconsumed_alarmed_queue() {
    panic!("{UNAVAILABLE}: force a durable effect failure through the configured retry policy");
}

#[test]
fn a_fanout_larger_than_one_batch_self_continues_until_quiescent() {
    panic!("{UNAVAILABLE}: insert more workspace projections than one configured worker batch");
}

#[test]
fn an_expired_lease_is_reclaimed_by_exactly_one_worker() {
    panic!("{UNAVAILABLE}: run two workers against one due operation");
}

#[test]
fn an_invitation_notification_is_sent_once_per_outbox_row() {
    panic!("{UNAVAILABLE}: deliver against a real sender identity and count messages");
}

#[test]
fn the_idempotency_sweep_removes_only_expired_records() {
    panic!("{UNAVAILABLE}: seed rows either side of the twenty-four hour boundary");
}

#[test]
fn the_worker_role_cannot_update_an_audit_or_epoch_row() {
    panic!("{UNAVAILABLE}: assert a real denial for both append-only tables");
}
