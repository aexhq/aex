//! Bounded local load contracts for secret route selection.

use regional_secret_api::secret_route_ids;

#[test]
fn secret_route_selection_stays_closed_under_load() {
    let expected = secret_route_ids();
    assert_eq!(expected.len(), 4);
    for _ in 0..10_000 {
        assert_eq!(secret_route_ids(), expected);
    }
}
