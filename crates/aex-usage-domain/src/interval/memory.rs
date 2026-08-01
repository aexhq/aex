//! The byte-millisecond integral.
//!
//! Memory is billed from an explicit reservation, never an RSS share. A resize
//! closes the current interval and opens a new one, so the integral over a
//! resized reservation equals the integral over the equivalent sequence of
//! fixed reservations exactly, with no rounding step anywhere.

use super::IntervalError;
use crate::quantity::Quantity;

/// The byte-millisecond quantity of one held interval.
///
/// # Errors
///
/// Returns [`IntervalError::Quantity`] when the product exceeds the storable
/// quantity ceiling.
pub fn byte_ms(bytes: u64, held_ms: u64) -> Result<Quantity, IntervalError> {
    Ok(Quantity::product(bytes, held_ms)?)
}

/// The byte-millisecond quantity of a sequence of `(bytes, held_ms)` segments.
///
/// # Errors
///
/// Returns [`IntervalError::Quantity`] when any segment or the running total
/// exceeds the storable quantity ceiling.
pub fn byte_ms_total(segments: &[(u64, u64)]) -> Result<Quantity, IntervalError> {
    let mut total = Quantity::ZERO;
    for (bytes, held_ms) in segments {
        total = total.checked_add(byte_ms(*bytes, *held_ms)?)?;
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::{byte_ms, byte_ms_total};
    use crate::quantity::Quantity;

    #[test]
    fn the_integral_is_the_product() {
        assert_eq!(
            byte_ms(4_096, 250).expect("fits"),
            Quantity::new(4_096 * 250).expect("fits")
        );
    }

    #[test]
    fn a_resized_reservation_integrates_to_the_same_total_as_fixed_segments() {
        // One reservation resized 1 KiB -> 2 KiB -> 512 B across 300 ms total.
        let resized = byte_ms_total(&[(1_024, 100), (2_048, 100), (512, 100)]).expect("fits");
        let fixed = byte_ms(1_024, 100)
            .expect("fits")
            .checked_add(byte_ms(2_048, 100).expect("fits"))
            .expect("fits")
            .checked_add(byte_ms(512, 100).expect("fits"))
            .expect("fits");
        assert_eq!(resized, fixed);
    }

    #[test]
    fn a_zero_length_hold_integrates_to_zero() {
        assert_eq!(byte_ms(1_048_576, 0).expect("fits"), Quantity::ZERO);
        assert_eq!(byte_ms(0, 1_000).expect("fits"), Quantity::ZERO);
    }

    #[test]
    fn an_unrepresentable_product_is_refused() {
        assert!(byte_ms(u64::MAX, u64::MAX).is_err());
    }
}
