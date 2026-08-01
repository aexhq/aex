//! The `central-identity-api` deployable against a real plane.
//!
//! Every case here needs a deployed identity plane and credentials. None of it
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
fn a_lost_commit_on_the_oauth_ceremony_replays_onto_the_same_person() {
    panic!("{UNAVAILABLE}: induce a real indeterminate commit and retry the preassigned user id");
}

#[test]
fn two_concurrent_first_sign_ins_for_one_provider_account_create_one_person() {
    panic!("{UNAVAILABLE}: race two callbacks against a live serializable transaction");
}

#[test]
fn a_redeemed_email_link_is_refused_at_every_later_instant() {
    panic!("{UNAVAILABLE}: redeem, then present the same link again and after a clock move");
}

#[test]
fn the_device_flow_completes_end_to_end_and_mints_exactly_one_token() {
    panic!("{UNAVAILABLE}: start, approve in a browser session, poll, and count the tokens");
}

#[test]
fn a_device_poll_faster_than_the_interval_is_told_to_slow_down() {
    panic!("{UNAVAILABLE}: poll twice inside the recorded interval against a live grant");
}

#[test]
fn disabling_a_person_advances_the_user_epoch_in_the_same_transaction() {
    panic!("{UNAVAILABLE}: disable, then read the epoch row inside the same commit");
}

#[test]
fn the_identity_role_cannot_write_any_control_table() {
    panic!("{UNAVAILABLE}: assert a real denial for every control table the role can see");
}

#[test]
fn the_public_device_routes_are_throttled_per_source_address() {
    panic!("{UNAVAILABLE}: drive the API Gateway usage plan past its per-IP burst");
}
