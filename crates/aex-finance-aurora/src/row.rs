//! Strict typed decoding at Aurora Data API boundaries.

use aex_finance_domain::{Microusd, MicrousdDelta, MoneyError};

use crate::wire_pending::RdsField;

/// Data API maximum bytes in one row.
pub const DATA_API_MAX_ROW_BYTES: usize = 64 * 1024;
/// Data API maximum response bytes.
pub const DATA_API_MAX_RESULT_BYTES: usize = 1024 * 1024;

/// Decodes a non-negative `BIGINT` money column.
///
/// # Errors
/// Rejects floats, null, non-canonical strings, negatives and business-bound overflow.
pub fn bigint_money(field: &RdsField) -> Result<Microusd, RowDecodeError> {
    match field {
        RdsField::Long(value) => Microusd::new(*value).map_err(RowDecodeError::Money),
        RdsField::String(value) => Microusd::parse_canonical(value).map_err(RowDecodeError::Money),
        RdsField::DoublePresent => Err(RowDecodeError::FloatInMoneyColumn),
        RdsField::Null => Err(RowDecodeError::Null),
    }
}

/// Decodes a signed `BIGINT` posting.
///
/// # Errors
/// Rejects floats, null and non-canonical signed strings.
pub fn bigint_delta(field: &RdsField) -> Result<MicrousdDelta, RowDecodeError> {
    match field {
        RdsField::Long(value) => MicrousdDelta::new(*value).map_err(RowDecodeError::Money),
        RdsField::String(value) => {
            let parsed = parse_signed(value)?;
            MicrousdDelta::new(parsed).map_err(RowDecodeError::Money)
        }
        RdsField::DoublePresent => Err(RowDecodeError::FloatInMoneyColumn),
        RdsField::Null => Err(RowDecodeError::Null),
    }
}

/// Decodes `NUMERIC(38,0)` exclusively from a canonical string.
///
/// # Errors
/// Rejects every non-string transport shape and non-canonical/overflowing text.
pub fn numeric_u128(field: &RdsField) -> Result<u128, RowDecodeError> {
    let RdsField::String(value) = field else {
        return if matches!(field, RdsField::DoublePresent) {
            Err(RowDecodeError::FloatInMoneyColumn)
        } else {
            Err(RowDecodeError::QuantityNotString)
        };
    };
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RowDecodeError::NonCanonicalInteger);
    }
    value.parse().map_err(|_| RowDecodeError::IntegerOverflow)
}

/// One bounded keyset page after typed decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowPage {
    /// Raw row bytes retained only for the protocol boundary test surface.
    pub rows: Vec<Vec<u8>>,
    /// Opaque keyset continuation; never an offset.
    pub next_key: Option<String>,
}

impl RowPage {
    /// Validates row and response bounds without truncation.
    ///
    /// # Errors
    /// Returns exact observed bytes for the first exceeded provider boundary.
    pub fn new(rows: Vec<Vec<u8>>, next_key: Option<String>) -> Result<Self, RowDecodeError> {
        for row in &rows {
            if row.len() > DATA_API_MAX_ROW_BYTES {
                return Err(RowDecodeError::RowTooLarge {
                    observed: row.len(),
                });
            }
        }
        let observed = rows.iter().try_fold(0_usize, |total, row| {
            total
                .checked_add(row.len())
                .ok_or(RowDecodeError::IntegerOverflow)
        })?;
        if observed > DATA_API_MAX_RESULT_BYTES {
            return Err(RowDecodeError::ResultTooLarge { observed });
        }
        Ok(Self { rows, next_key })
    }
}

fn parse_signed(value: &str) -> Result<i64, RowDecodeError> {
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty()
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(RowDecodeError::NonCanonicalInteger);
    }
    value.parse().map_err(|_| RowDecodeError::IntegerOverflow)
}

/// Typed row failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowDecodeError {
    /// Money was encoded through a floating transport member.
    #[error("floating point is forbidden in a finance column")]
    FloatInMoneyColumn,
    /// Required value was null.
    #[error("finance column is unexpectedly null")]
    Null,
    /// Quantity did not use the string member.
    #[error("NUMERIC(38,0) quantity must be a string value")]
    QuantityNotString,
    /// Integer text was not canonical.
    #[error("integer text is not canonical")]
    NonCanonicalInteger,
    /// Integer or length arithmetic overflowed.
    #[error("integer value overflowed")]
    IntegerOverflow,
    /// Domain money bound failed.
    #[error(transparent)]
    Money(MoneyError),
    /// One row exceeded 64 KiB.
    #[error("Data API row is too large: {observed} bytes")]
    RowTooLarge {
        /// Observed bytes.
        observed: usize,
    },
    /// One response exceeded 1 MiB.
    #[error("Data API result is too large: {observed} bytes")]
    ResultTooLarge {
        /// Observed bytes.
        observed: usize,
    },
}
