//! Public exact-accumulation identity evidence.

use aex_usage_rating::accumulate;
use num_traits::Zero as _;

#[test]
fn empty_exact_accumulation_is_the_additive_identity() {
    assert!(accumulate([]).is_zero());
}
