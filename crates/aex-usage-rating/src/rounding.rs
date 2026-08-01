//! The single exact-rational to micro-USD settlement boundary.

use aex_finance_domain::Microusd;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Signed as _, ToPrimitive as _, Zero as _};

use crate::exact::{RatedSegment, SegmentKey};
use crate::rate_card::{RateContext, RatingError, RoundingRule};

/// Rounds one fully accumulated segment exactly once.
///
/// # Errors
/// Rejects negative totals and values outside the finance amount bound.
pub fn settle_segment(
    context: &RateContext,
    key: SegmentKey,
    exact: BigRational,
) -> Result<RatedSegment, RatingError> {
    let rounded = rational_to_microusd(&exact, context.rounding())?;
    let residual = exact.clone() - BigRational::from_integer(BigInt::from(rounded.get()));
    Ok(RatedSegment {
        key,
        exact,
        rounded,
        residual,
        book: context.book_id(),
    })
}

fn rational_to_microusd(exact: &BigRational, rule: RoundingRule) -> Result<Microusd, RatingError> {
    if exact.is_negative() {
        return Err(RatingError::NegativeAmount);
    }
    let numerator = exact.numer();
    let denominator = exact.denom();
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let rounded = match rule {
        RoundingRule::CeilTotal => {
            if remainder.is_zero() {
                quotient
            } else {
                quotient + 1
            }
        }
        RoundingRule::HalfEven => {
            let doubled = &remainder * 2;
            if doubled > *denominator
                || (doubled == *denominator && (&quotient % 2) != BigInt::zero())
            {
                quotient + 1
            } else {
                quotient
            }
        }
    };
    let value = rounded
        .to_i64()
        .ok_or(RatingError::AmountExceedsBusinessBound)?;
    Microusd::new(value).map_err(|_| RatingError::AmountExceedsBusinessBound)
}
