//! Internal money.
//!
//! One cent is exactly ten thousand micro-USD, so the conversion is lossless in
//! one direction and checked in the other. `Microusd` is unsigned and
//! `MicrousdDelta` is the only type in the workspace that may be negative, which
//! makes "a balance went below zero" a type error at the call site rather than a
//! reconciliation surprise a month later.

use aex_wire::types::Cents;
use serde::{Deserialize, Serialize};

/// Micro-USD per cent.
pub const MICROUSD_PER_CENT: u64 = 10_000;

/// A money arithmetic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// The result did not fit its type.
    #[error("micro-USD arithmetic overflowed")]
    Overflow,
    /// The result would have been negative.
    #[error("micro-USD arithmetic went below zero")]
    Negative,
}

/// A non-negative micro-USD amount.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Microusd(u64);

impl Microusd {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Wraps a raw micro-USD amount.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw micro-USD amount.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Converts whole cents exactly.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] when the product exceeds `u64`.
    pub const fn from_cents(cents: Cents) -> Result<Self, MoneyError> {
        match cents.get().checked_mul(MICROUSD_PER_CENT) {
            Some(value) => Ok(Self(value)),
            None => Err(MoneyError::Overflow),
        }
    }

    /// Converts to whole cents, rejecting any amount that is not a whole cent.
    ///
    /// Rounding here would be a silent transfer of money, so it is refused.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] when the amount is not a whole number of
    /// cents.
    pub const fn to_cents_exact(self) -> Result<Cents, MoneyError> {
        if !self.0.is_multiple_of(MICROUSD_PER_CENT) {
            return Err(MoneyError::Overflow);
        }
        Ok(Cents::new(self.0 / MICROUSD_PER_CENT))
    }

    /// Applies a signed delta.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] or [`MoneyError::Negative`].
    pub const fn checked_add(self, delta: MicrousdDelta) -> Result<Self, MoneyError> {
        if delta.0 >= 0 {
            match self.0.checked_add(delta.0.unsigned_abs()) {
                Some(value) => Ok(Self(value)),
                None => Err(MoneyError::Overflow),
            }
        } else {
            match self.0.checked_sub(delta.0.unsigned_abs()) {
                Some(value) => Ok(Self(value)),
                None => Err(MoneyError::Negative),
            }
        }
    }
}

/// A signed micro-USD delta. The only money type that may be negative.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct MicrousdDelta(i64);

impl MicrousdDelta {
    /// Wraps a raw signed micro-USD amount.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// The raw signed micro-USD amount.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// The delta that adds `amount`.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] when the amount exceeds `i64`.
    pub fn credit(amount: Microusd) -> Result<Self, MoneyError> {
        match i64::try_from(amount.0) {
            Ok(value) => Ok(Self(value)),
            Err(_) => Err(MoneyError::Overflow),
        }
    }

    /// The delta that removes `amount`.
    ///
    /// # Errors
    ///
    /// Returns [`MoneyError::Overflow`] when the amount exceeds `i64`.
    pub fn debit(amount: Microusd) -> Result<Self, MoneyError> {
        match i64::try_from(amount.0) {
            Ok(value) => Ok(Self(-value)),
            Err(_) => Err(MoneyError::Overflow),
        }
    }
}
