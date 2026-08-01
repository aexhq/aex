//! Integer-only money values and checked conversions.

use serde::{Deserialize, Serialize};

/// One cent in micro-USD.
pub const MICROUSD_PER_CENT: i64 = 10_000;

/// A money parsing or arithmetic failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    /// A non-negative type was given a negative value.
    #[error("money cannot be negative")]
    Negative,
    /// The value is beyond the business or representation bound.
    #[error("money exceeds its business bound")]
    Overflow,
    /// The text was not a canonical base-ten integer.
    #[error("money must be a canonical base-ten integer")]
    NonCanonical,
    /// A micro-USD value was not an exact whole number of cents.
    #[error("micro-USD amount is not an exact number of cents")]
    ScaleLoss,
}

/// A non-negative whole-cent amount on payment surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cents(i64);

impl Cents {
    /// Largest accepted amount: one hundred billion cents.
    pub const MAX_RAW: i64 = 100_000_000_000;

    /// Constructs a bounded amount.
    ///
    /// # Errors
    /// Returns [`MoneyError::Negative`] or [`MoneyError::Overflow`].
    pub const fn new(value: i64) -> Result<Self, MoneyError> {
        if value < 0 {
            Err(MoneyError::Negative)
        } else if value > Self::MAX_RAW {
            Err(MoneyError::Overflow)
        } else {
            Ok(Self(value))
        }
    }

    /// Parses the canonical integer wire spelling.
    ///
    /// # Errors
    /// Returns a typed error for non-canonical or out-of-range input.
    pub fn parse_decimal(text: &str) -> Result<Self, MoneyError> {
        parse_nonnegative(text).and_then(Self::new)
    }

    /// Returns the raw cents.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Converts cents to micro-USD exactly.
    #[must_use]
    pub const fn to_microusd(self) -> Microusd {
        Microusd(self.0 * MICROUSD_PER_CENT)
    }
}

/// A non-negative internal micro-USD amount.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Microusd(i64);

impl Microusd {
    /// Zero micro-USD.
    pub const ZERO: Self = Self(0);
    /// Maximum single amount or posting magnitude.
    pub const MAX: Self = Self(Self::MAX_RAW);
    /// Raw maximum value.
    pub const MAX_RAW: i64 = 1_000_000_000_000_000;

    /// Constructs a bounded non-negative amount.
    ///
    /// # Errors
    /// Returns [`MoneyError::Negative`] or [`MoneyError::Overflow`].
    pub const fn new(value: i64) -> Result<Self, MoneyError> {
        if value < 0 {
            Err(MoneyError::Negative)
        } else if value > Self::MAX_RAW {
            Err(MoneyError::Overflow)
        } else {
            Ok(Self(value))
        }
    }

    /// Parses the canonical integer wire spelling.
    ///
    /// # Errors
    /// Returns a typed error for non-canonical or out-of-range input.
    pub fn parse_canonical(text: &str) -> Result<Self, MoneyError> {
        parse_nonnegative(text).and_then(Self::new)
    }

    /// Returns the raw micro-USD.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Adds two amounts within the business bound.
    ///
    /// # Errors
    /// Returns [`MoneyError::Overflow`] if the sum exceeds the bound.
    pub const fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        match self.0.checked_add(other.0) {
            Some(value) => Self::new(value),
            None => Err(MoneyError::Overflow),
        }
    }

    /// Subtracts without permitting a negative result.
    ///
    /// # Errors
    /// Returns [`MoneyError::Negative`] on underflow.
    pub const fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        match self.0.checked_sub(other.0) {
            Some(value) => Self::new(value),
            None => Err(MoneyError::Negative),
        }
    }

    /// Converts to a debit-positive signed amount.
    #[must_use]
    pub const fn as_delta(self) -> MicrousdDelta {
        MicrousdDelta(self.0)
    }

    /// Converts to whole cents without rounding.
    ///
    /// # Errors
    /// Returns [`MoneyError::ScaleLoss`] for a fractional cent.
    pub const fn to_cents_exact(self) -> Result<Cents, MoneyError> {
        if self.0 % MICROUSD_PER_CENT != 0 {
            return Err(MoneyError::ScaleLoss);
        }
        Cents::new(self.0 / MICROUSD_PER_CENT)
    }
}

/// A signed debit-positive journal posting.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct MicrousdDelta(i64);

impl MicrousdDelta {
    /// Zero signed micro-USD.
    pub const ZERO: Self = Self(0);
    /// Largest permitted magnitude.
    pub const MAX_ABS: i64 = Microusd::MAX_RAW;

    /// Constructs a bounded signed posting.
    ///
    /// # Errors
    /// Returns [`MoneyError::Overflow`] outside the posting bound.
    pub const fn new(value: i64) -> Result<Self, MoneyError> {
        if value < -Self::MAX_ABS || value > Self::MAX_ABS {
            Err(MoneyError::Overflow)
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the raw signed micro-USD amount.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Returns the exact negation.
    #[must_use]
    pub const fn negate(self) -> Self {
        Self(-self.0)
    }
}

fn parse_nonnegative(text: &str) -> Result<i64, MoneyError> {
    if text.is_empty()
        || (text.len() > 1 && text.starts_with('0'))
        || !text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(MoneyError::NonCanonical);
    }
    text.parse::<i64>().map_err(|_| MoneyError::Overflow)
}
