//! Partial-batch fail-closed evidence.

use aex_finance_app::use_cases::partial_batch_failures;

#[test]
fn an_empty_delivery_never_invents_a_batch_failure() {
    assert!(partial_batch_failures(&[], &[]).is_empty());
}
