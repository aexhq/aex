//! Exact contribution arithmetic before the settlement boundary.

use aex_finance_domain::Microusd;
use aex_internal_contracts::usage::{FactId, Meter};
use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::Zero as _;
use serde::{Deserialize, Serialize};

use crate::rate_card::{RateContext, RatingError};

/// Content digest of one immutable rate book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RateBookId([u8; 32]);

impl RateBookId {
    /// Wraps a digest after verification.
    #[must_use]
    pub const fn new(value: [u8; 32]) -> Self {
        Self(value)
    }

    /// Digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// One fact's exact contribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contribution {
    /// Meter used for rating.
    pub meter: Meter,
    /// Deterministic fact identity.
    pub fact: FactId,
    /// Exact unrounded micro-USD.
    pub exact: BigRational,
}

/// Settlement segment identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentKey {
    /// Organization key, opaque here because account authority validates it.
    pub organization: String,
    /// Reservation key.
    pub reservation: String,
    /// Priced meter.
    pub meter: Meter,
}

/// Once-rounded settlement segment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RatedSegment {
    /// Segment identity.
    pub key: SegmentKey,
    /// Exact accumulated amount.
    pub exact: BigRational,
    /// Single rounded amount.
    pub rounded: Microusd,
    /// `exact - rounded`, retained for audit.
    pub residual: BigRational,
    /// Immutable book identity.
    pub book: RateBookId,
}

/// Rates a base-unit quantity exactly, without rounding.
///
/// # Errors
/// Returns [`RatingError::MeterNotPriced`] if the verified book lacks the meter.
pub fn rate_quantity(
    context: &RateContext,
    meter: Meter,
    base_units: u128,
    fact: FactId,
) -> Result<Contribution, RatingError> {
    let rate = context.rate(meter)?;
    Ok(Contribution {
        meter,
        fact,
        exact: rate.exact() * BigInt::from(base_units),
    })
}

/// Accumulates exact values in any delivery order.
#[must_use]
pub fn accumulate(parts: impl IntoIterator<Item = Contribution>) -> BigRational {
    parts
        .into_iter()
        .fold(BigRational::zero(), |total, part| total + part.exact)
}
