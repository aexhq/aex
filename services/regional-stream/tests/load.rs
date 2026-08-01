//! Bounded local load contracts for authoritative stream scans.

use regional_stream::authoritative_after_wakes;

#[test]
fn authoritative_cursor_scan_is_linear_at_the_page_bound() {
    let authority = (0..200_000).collect::<Vec<_>>();
    let selected = authoritative_after_wakes(&authority, 99_999, &[u64::MAX; 1_000]);
    assert_eq!(selected.len(), 100_000);
    assert_eq!(selected.first(), Some(&100_000));
    assert_eq!(selected.last(), Some(&199_999));
}
