//! Exact integer quantities.
//!
//! There is no floating-point value anywhere in the usage authority. A quantity
//! is a `u128` capped below the DynamoDB `N` significant-digit ceiling, so a
//! representable quantity can never be silently truncated by the row codec.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The largest representable quantity: `10^38 - 1`.
///
/// DynamoDB's `N` type carries 38 significant digits and `u128::MAX` has 39, so
/// the ceiling is enforced at construction rather than discovered at write time.
pub const MAX_QUANTITY: u128 = 10u128.pow(38) - 1;

/// Why a quantity was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuantityError {
    /// The value exceeded [`MAX_QUANTITY`].
    #[error("quantity {value} exceeds the {MAX_QUANTITY} storable ceiling")]
    OutOfRange {
        /// The value that was refused.
        value: u128,
    },
    /// An arithmetic step overflowed `u128` before the ceiling could be checked.
    #[error("quantity arithmetic overflowed: {operation}")]
    Overflow {
        /// The operation that overflowed.
        operation: &'static str,
    },
    /// The decimal text was not a bare non-negative integer.
    #[error("`{value}` is not a bare non-negative decimal integer")]
    Malformed {
        /// The text that was refused.
        value: String,
    },
}

/// An exact, non-negative integer measurement in a meter's base unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Quantity(u128);

impl Quantity {
    /// The additive identity.
    pub const ZERO: Self = Self(0);

    /// Builds a quantity, refusing anything above [`MAX_QUANTITY`].
    ///
    /// # Errors
    ///
    /// Returns [`QuantityError::OutOfRange`] when `value > MAX_QUANTITY`.
    pub const fn new(value: u128) -> Result<Self, QuantityError> {
        if value > MAX_QUANTITY {
            return Err(QuantityError::OutOfRange { value });
        }
        Ok(Self(value))
    }

    /// The underlying integer.
    #[must_use]
    pub const fn get(self) -> u128 {
        self.0
    }

    /// Whether this quantity is exactly zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Adds two quantities, refusing overflow and the storable ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`QuantityError::Overflow`] on `u128` overflow and
    /// [`QuantityError::OutOfRange`] when the sum exceeds [`MAX_QUANTITY`].
    pub const fn checked_add(self, other: Self) -> Result<Self, QuantityError> {
        match self.0.checked_add(other.0) {
            Some(sum) => Self::new(sum),
            None => Err(QuantityError::Overflow { operation: "add" }),
        }
    }

    /// Subtracts `other`, refusing a negative result.
    ///
    /// # Errors
    ///
    /// Returns [`QuantityError::Overflow`] when `other` exceeds `self`; a usage
    /// quantity is never negative, so the caller has a correction ordering bug.
    pub const fn checked_sub(self, other: Self) -> Result<Self, QuantityError> {
        match self.0.checked_sub(other.0) {
            Some(difference) => Ok(Self(difference)),
            None => Err(QuantityError::Overflow {
                operation: "subtract",
            }),
        }
    }

    /// Multiplies two `u64` factors into a quantity.
    ///
    /// Every meter's quantum is a product of two `u64` measurements, so this is
    /// the only multiplication the evidence rules need.
    ///
    /// # Errors
    ///
    /// Returns [`QuantityError::OutOfRange`] when the product exceeds
    /// [`MAX_QUANTITY`].
    pub const fn product(left: u64, right: u64) -> Result<Self, QuantityError> {
        Self::new(left as u128 * right as u128)
    }

    /// Parses the canonical decimal form written to DynamoDB's `N` type.
    ///
    /// # Errors
    ///
    /// Returns [`QuantityError::Malformed`] for anything that is not a bare
    /// non-negative decimal integer, and [`QuantityError::OutOfRange`] above
    /// [`MAX_QUANTITY`].
    pub fn parse(value: &str) -> Result<Self, QuantityError> {
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(QuantityError::Malformed {
                value: value.to_owned(),
            });
        }
        if value.len() > 1 && value.starts_with('0') {
            return Err(QuantityError::Malformed {
                value: value.to_owned(),
            });
        }
        let parsed = value
            .parse::<u128>()
            .map_err(|_| QuantityError::OutOfRange { value: MAX_QUANTITY })?;
        Self::new(parsed)
    }
}

impl fmt::Display for Quantity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl TryFrom<String> for Quantity {
    type Error = QuantityError;

    fn try_from(value: String) -> Result<Self, QuantityError> {
        Self::parse(&value)
    }
}

impl From<Quantity> for String {
    fn from(value: Quantity) -> Self {
        value.0.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_QUANTITY, Quantity, QuantityError};

    #[test]
    fn refuses_above_the_storable_ceiling() {
        assert!(Quantity::new(MAX_QUANTITY).is_ok());
        assert_eq!(
            Quantity::new(MAX_QUANTITY + 1),
            Err(QuantityError::OutOfRange {
                value: MAX_QUANTITY + 1
            })
        );
    }

    #[test]
    fn addition_is_checked_at_both_ceilings() {
        let big = Quantity::new(MAX_QUANTITY).expect("ceiling is representable");
        let one = Quantity::new(1).expect("one is representable");
        assert!(matches!(
            big.checked_add(one),
            Err(QuantityError::OutOfRange { .. })
        ));
        assert_eq!(
            Quantity::ZERO.checked_add(one).expect("no overflow"),
            Quantity::new(1).expect("one")
        );
    }

    #[test]
    fn subtraction_refuses_a_negative_result() {
        let one = Quantity::new(1).expect("one");
        assert!(matches!(
            Quantity::ZERO.checked_sub(one),
            Err(QuantityError::Overflow { .. })
        ));
    }

    #[test]
    fn decimal_text_round_trips_and_rejects_hostile_forms() {
        let value = Quantity::new(1_234_567_890_123_456_789).expect("representable");
        assert_eq!(
            Quantity::parse(&value.to_string()).expect("round trip"),
            value
        );
        for hostile in ["", "-1", "01", "1.0", "1e3", " 1", "+1", "1_000"] {
            assert!(
                Quantity::parse(hostile).is_err(),
                "`{hostile}` must be refused"
            );
        }
    }

    #[test]
    fn products_of_two_u64_factors_are_bounded() {
        assert_eq!(
            Quantity::product(3, 4).expect("small product"),
            Quantity::new(12).expect("twelve")
        );
        assert!(Quantity::product(u64::MAX, u64::MAX).is_err());
    }
}
