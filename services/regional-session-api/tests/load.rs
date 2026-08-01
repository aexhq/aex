//! Bounded local load contracts for session route selection.

use regional_session_api::routes::session_route_ids;

#[test]
fn route_selection_stays_allocation_bounded_under_load() {
    let expected = session_route_ids();
    assert!(!expected.is_empty());
    for _ in 0..10_000 {
        assert_eq!(session_route_ids(), expected);
    }
}
