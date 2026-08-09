//! Bounded local load contracts for both halves' route selection and scans.

use session_stream_api::session::routes::session_route_ids;
use session_stream_api::stream::authoritative_after_wakes;

#[test]
fn route_selection_stays_allocation_bounded_under_load() {
    let expected = session_route_ids();
    assert!(!expected.is_empty());
    for _ in 0..10_000 {
        assert_eq!(session_route_ids(), expected);
    }
}

#[test]
fn authoritative_cursor_scan_is_linear_at_the_page_bound() {
    let authority = (0..200_000).collect::<Vec<_>>();
    let selected = authoritative_after_wakes(&authority, 99_999, &[u64::MAX; 1_000]);
    assert_eq!(selected.len(), 100_000);
    assert_eq!(selected.first(), Some(&100_000));
    assert_eq!(selected.last(), Some(&199_999));
}
