//! The `central-control-api` deployable against a real plane.
//!
//! Every case here needs a deployed control plane and credentials. None of it
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
fn a_workspace_is_returned_only_once_both_authorities_are_durable() {
    panic!("{UNAVAILABLE}: provision against a real region and assert the hidden window");
}

#[test]
fn a_lost_regional_response_leaves_the_workspace_hidden_and_retryable() {
    panic!("{UNAVAILABLE}: drop the regional reply and retry the same Idempotency-Key");
}

#[test]
fn a_region_answering_about_another_workspace_is_refused() {
    panic!("{UNAVAILABLE}: return a foreign workspace id from a real regional authority");
}

#[test]
fn deleting_a_workspace_revokes_every_key_before_any_regional_cleanup() {
    panic!("{UNAVAILABLE}: delete, then present each key to a regional edge");
}

#[test]
fn a_keyset_page_walks_a_thousand_rows_without_repeating_or_dropping_one() {
    panic!("{UNAVAILABLE}: seed 1000 rows and walk every page through the signed cursor");
}

#[test]
fn a_cursor_minted_for_one_principal_is_refused_for_another() {
    panic!("{UNAVAILABLE}: replay a real cursor under a second credential");
}

#[test]
fn removing_the_last_owner_is_refused_by_the_deferred_constraint() {
    panic!("{UNAVAILABLE}: race two removals against a live deferred constraint trigger");
}

#[test]
fn the_control_role_cannot_reach_a_finance_object() {
    panic!("{UNAVAILABLE}: assert a real denial for every finance table and function");
}
