//! The `tool-executor` deployable against a real plane.
//!
//! Every case here needs the executor deployed, `brain-mux` deployed beside it
//! and the ceiling table to exist. None of that can be earned in this run
//! (OD-07), so each test **fails loudly** when the live lane selects it rather
//! than self-skipping: a green suite that ran nothing is worse than a red one
//! that says why.
//!
//! Two of these are the ones that matter most, because they are the two claims
//! this design makes that a unit test cannot reach. The first is that the
//! executor has no address a customer could name. The second is that the
//! conditional update refuses rather than accumulates -- the exact failure the
//! OTLP quota next door has been shipping since it was written.

/// The message every unavailable live case fails with.
const UNAVAILABLE: &str =
    "live evidence requires a deployed plane and credentials; this run deploys nothing (OD-07)";

#[test]
fn the_executor_resolves_only_inside_the_vpc_and_answers_nothing_from_the_internet() {
    panic!(
        "{UNAVAILABLE}: resolve the Cloud Map name from outside the VPC and prove NXDOMAIN, then \
         prove the task has no public address and no listener rule anywhere"
    );
}

#[test]
fn the_organization_ceiling_refuses_rather_than_accumulating() {
    panic!(
        "{UNAVAILABLE}: drive one organization past its per-minute ceiling against the real table \
         and assert the transaction is cancelled, then read the item and assert the count stopped \
         at the ceiling instead of running past it"
    );
}

#[test]
fn a_refusal_on_the_day_window_leaves_the_minute_window_uncharged() {
    panic!(
        "{UNAVAILABLE}: fill the day window, attempt one call, and read both items -- the minute \
         count must not have moved"
    );
}

#[test]
fn a_brain_minted_envelope_verifies_against_the_deployed_key_set() {
    panic!(
        "{UNAVAILABLE}: sign in brain-mux with the deployed kid and present it to the deployed \
         executor over the private name"
    );
}

#[test]
fn an_envelope_minted_for_another_attempt_is_refused() {
    panic!(
        "{UNAVAILABLE}: capture one request in flight and replay its envelope against attempt+1"
    );
}

#[test]
fn the_task_role_is_denied_every_store_it_does_not_need() {
    panic!(
        "{UNAVAILABLE}: assert a real IAM denial for the session, custody and keystore tables from \
         the executor's own task role"
    );
}

#[test]
fn a_deploy_stalls_no_in_flight_call_for_longer_than_one_deadline() {
    panic!(
        "{UNAVAILABLE}: hold a call open, roll the service, and measure how long the caller's \
         activation stayed blocked -- one task means this is the availability cost the owner \
         accepted, and it should be measured rather than assumed"
    );
}
