//! Millisecond arithmetic over the wire [`Timestamp`].
//!
//! The true-idle boundary is defined at exactly 180000 ms, so elapsed time is
//! computed in whole milliseconds and never in a floating or nanosecond-backed
//! representation where `179_999` versus `180_000` becomes a rounding question.
//!
//! Both helpers saturate. A clock that moves backwards must never present as a
//! huge elapsed interval — that would make a busy generation look idle — and an
//! instant pushed past the representable wire range must clamp rather than fail a
//! decision that has nothing to do with formatting.

use aex_wire::types::Timestamp;

/// The largest instant the wire spelling can express.
///
/// Mirrored here because `aex-wire` keeps the bound private; the constructor
/// rejects anything outside it, and this crate needs to clamp rather than error.
const MAX_MILLIS: i64 = 253_402_300_799_999;

/// The smallest instant the wire spelling can express.
const MIN_MILLIS: i64 = -62_167_219_200_000;

/// Milliseconds elapsed from `earlier` to `now`, saturating at zero.
///
/// Saturation is the point: a backwards clock yields `0`, which makes the
/// generation look *busy*, never idle. Erring toward busy costs money; erring
/// toward idle discards a customer's running work.
#[must_use]
pub const fn millis_between(earlier: Timestamp, now: Timestamp) -> u64 {
    let delta = now.unix_millis().saturating_sub(earlier.unix_millis());
    if delta <= 0 { 0 } else { delta.unsigned_abs() }
}

/// `at` advanced by `millis`, clamped to the representable wire range.
#[must_use]
pub fn plus_millis(at: Timestamp, millis: u64) -> Timestamp {
    let raw = at.unix_millis().saturating_add_unsigned(millis);
    let clamped = if raw > MAX_MILLIS { MAX_MILLIS } else { raw };
    Timestamp::from_unix_millis(clamped).unwrap_or(at)
}

/// `at` moved back by `millis`, clamped to the representable wire range.
#[must_use]
pub fn minus_millis(at: Timestamp, millis: u64) -> Timestamp {
    let raw = at.unix_millis().saturating_sub_unsigned(millis);
    let clamped = if raw < MIN_MILLIS { MIN_MILLIS } else { raw };
    Timestamp::from_unix_millis(clamped).unwrap_or(at)
}

#[cfg(test)]
mod tests {
    use super::{millis_between, minus_millis, plus_millis};
    use aex_wire::types::Timestamp;

    fn at(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("a bounded instant")
    }

    #[test]
    fn elapsed_saturates_when_the_clock_moves_backwards() {
        assert_eq!(millis_between(at(4_000), at(1_000)), 0);
    }

    #[test]
    fn elapsed_is_exact_at_the_idle_boundary() {
        assert_eq!(millis_between(at(0), at(179_999)), 179_999);
        assert_eq!(millis_between(at(0), at(180_000)), 180_000);
    }

    #[test]
    fn advancing_past_the_representable_range_clamps_instead_of_wrapping() {
        let far = plus_millis(at(0), u64::MAX);
        assert_eq!(far.unix_millis(), 253_402_300_799_999);
        assert_eq!(plus_millis(far, 1), far);
    }

    #[test]
    fn moving_back_past_the_representable_range_clamps_instead_of_wrapping() {
        let far = minus_millis(at(0), u64::MAX);
        assert_eq!(far.unix_millis(), -62_167_219_200_000);
        assert_eq!(minus_millis(far, 1), far);
    }
}
