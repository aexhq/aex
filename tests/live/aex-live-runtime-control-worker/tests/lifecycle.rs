//! The runtime control worker against a real `MicroVM` control plane.
//!
//! Every case here needs a deployed plane, AWS credentials and provider quota.
//! None of it can be earned in this run (OD-07), so each test **fails loudly**
//! when the live lane selects it rather than self-skipping.
//!
//! The pure halves already pass in the default lane: the exact 179999/180000 ms
//! boundary, the fence algebra, the lifetime margins, the pressure ranking, the
//! shape-to-meter table and the fact derivation are all decided by
//! `aex-runtime-control` with no clock and no I/O. What is unavailable here is
//! the part that needs a provider to actually do something.

/// The message every unavailable live case fails with.
const UNAVAILABLE: &str =
    "live evidence requires a deployed plane and provider quota; this run deploys nothing (OD-07)";

#[test]
fn a_busy_generation_is_not_suspended_at_the_boundary_and_an_idle_one_is() {
    panic!("{UNAVAILABLE}: hold an open background operation across 180000 ms, then release it");
}

#[test]
fn a_suspend_racing_new_hands_work_admits_exactly_one_side() {
    panic!("{UNAVAILABLE}: real conditional writes against the runtime-activity table");
}

#[test]
fn a_worker_crash_at_every_transition_point_leaves_a_reconcilable_state() {
    panic!("{UNAVAILABLE}: kill after the lock, the intent, the dispatch, and the receipt");
}

#[test]
fn an_unknown_provider_outcome_reconciles_by_exact_microvm_identity() {
    panic!("{UNAVAILABLE}: induce a real indeterminate outcome and reconcile it");
}

#[test]
fn the_eight_hour_lifetime_drains_and_terminates_on_time() {
    panic!("{UNAVAILABLE}: an eight-hour generation, drained at -300 s and terminated at -60 s");
}

#[test]
fn every_compute_shape_produces_the_exact_metered_quantity() {
    panic!("{UNAVAILABLE}: run all five shapes and reconcile against the provider interval");
}

#[test]
fn a_redelivered_lifecycle_event_does_not_double_bill() {
    panic!("{UNAVAILABLE}: redeliver a settled lifecycle message against a real authority");
}

#[test]
fn the_worker_role_cannot_reach_a_forbidden_action() {
    panic!("{UNAVAILABLE}: assert a real IAM denial for CreateMicrovmImage and the shell token");
}

#[test]
fn the_provider_exposes_or_does_not_expose_per_generation_transmit_bytes() {
    panic!("{UNAVAILABLE}: the probe that decides whether the transfer meter can ever turn on");
}

#[test]
fn the_provider_exposes_or_does_not_expose_a_snapshot_size() {
    panic!("{UNAVAILABLE}: the probe that decides whether the declared value stays authoritative");
}
