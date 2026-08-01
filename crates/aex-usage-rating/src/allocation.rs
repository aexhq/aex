//! Deterministic largest-remainder allocation of a rounded segment.

use aex_finance_domain::Microusd;
use aex_internal_contracts::usage::FactId;
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{ToPrimitive as _, Zero as _};

use crate::exact::{Contribution, RatedSegment};
use crate::rate_card::RatingError;

/// Allocates the already-rounded segment total without re-rating a fact.
///
/// # Errors
/// Rejects contributions from another meter, a mismatched exact total, duplicate
/// facts, or values outside the finance amount bound.
pub fn allocate_segment(
    segment: &RatedSegment,
    parts: &[Contribution],
) -> Result<Vec<(FactId, Microusd)>, RatingError> {
    if parts.iter().any(|part| part.meter != segment.key.meter) {
        return Err(RatingError::MalformedRateBook);
    }
    let sum = parts
        .iter()
        .fold(BigRational::zero(), |total, part| total + &part.exact);
    if sum != segment.exact {
        return Err(RatingError::QuantityOverflow);
    }
    if sum.is_zero() {
        return parts
            .iter()
            .map(|part| Ok((part.fact, Microusd::ZERO)))
            .collect();
    }

    let total = BigInt::from(segment.rounded.get());
    let mut rows = Vec::with_capacity(parts.len());
    let mut floors = 0_i64;
    for part in parts {
        let quota = &part.exact * &total / &sum;
        let floor = quota.numer() / quota.denom();
        let floor_i64 = floor
            .to_i64()
            .ok_or(RatingError::AmountExceedsBusinessBound)?;
        floors = floors
            .checked_add(floor_i64)
            .ok_or(RatingError::AmountExceedsBusinessBound)?;
        rows.push((
            part.fact,
            floor_i64,
            quota - BigRational::from_integer(floor),
        ));
    }
    let remainder = segment
        .rounded
        .get()
        .checked_sub(floors)
        .ok_or(RatingError::QuantityOverflow)?;
    rows.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));
    let remainder = usize::try_from(remainder).map_err(|_| RatingError::QuantityOverflow)?;
    if remainder > rows.len() {
        return Err(RatingError::QuantityOverflow);
    }
    for row in rows.iter_mut().take(remainder) {
        row.1 += 1;
    }
    rows.sort_by_key(|row| row.0);
    rows.into_iter()
        .map(|(fact, value, _)| {
            Microusd::new(value)
                .map(|amount| (fact, amount))
                .map_err(|_| RatingError::AmountExceedsBusinessBound)
        })
        .collect()
}
