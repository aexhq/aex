//! The `central-authz` deployable against a real plane.
//!
//! Every case here needs a deployed authorization plane and credentials. None of it
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
fn a_workspace_key_resolves_in_exactly_one_data_api_statement() {
    panic!("{UNAVAILABLE}: count the real Data API calls for one issue against a live cluster");
}

#[test]
fn an_account_token_and_a_browser_session_yield_the_same_envelope_shape() {
    panic!("{UNAVAILABLE}: issue both against one workspace and diff the 323 bytes field by field");
}

#[test]
fn a_revoked_key_stops_verifying_within_the_thirty_second_window() {
    panic!("{UNAVAILABLE}: revoke, then poll a regional edge until it refuses");
}

#[test]
fn an_advanced_epoch_refuses_an_assertion_issued_before_it() {
    panic!("{UNAVAILABLE}: bump the account epoch and replay an in-flight assertion");
}

#[test]
fn the_read_only_role_is_denied_every_write_it_could_attempt() {
    panic!("{UNAVAILABLE}: assert a real IAM and PostgreSQL denial for each granted table");
}

#[test]
fn an_absent_finance_row_answers_account_state_unavailable_not_active() {
    panic!("{UNAVAILABLE}: drop the finance view and observe the 503");
}

#[test]
fn a_rotated_signing_key_is_accepted_by_every_region_before_the_old_one_retires() {
    panic!("{UNAVAILABLE}: rotate and verify against all five regional verification key sets");
}

#[test]
fn a_five_hundred_credential_burst_collapses_to_one_statement_per_window() {
    panic!("{UNAVAILABLE}: drive the Area-10 refresh table and count statements");
}
