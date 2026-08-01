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
fn a_redelivered_fifo_message_reconciles_rather_than_dispatching_twice() {
    panic!("{UNAVAILABLE}: redeliver a settled provisioning message against a live region");
}

#[test]
fn a_partial_batch_names_only_the_items_that_did_not_commit() {
    panic!("{UNAVAILABLE}: fail one item of a real batch and inspect the response");
}

#[test]
fn a_poison_intent_conflict_quarantines_to_the_dead_letter_queue() {
    panic!("{UNAVAILABLE}: replay a changed intent under a live operation id");
}

#[test]
fn a_scheduled_due_scan_recovers_an_operation_whose_queue_hint_was_lost() {
    panic!("{UNAVAILABLE}: delete the queue message and wait for the schedule");
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
