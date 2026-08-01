//! Strict typed decoding at Aurora Data API boundaries.
//!
//! [`aex_rds_data::Record`] is the authority on what a Data `API` field is: it
//! publishes indexed accessors rather than a field-value enum, and it has no
//! floating-point decode path at all. These decoders sit directly on it and add
//! only the finance-specific bounds, so there is no second opinion about which
//! transport shapes a money column may arrive in.
//!
//! The schema decides which accessor applies. `finance.journal_posting`'s
//! `amount_microusd` and `finance.account_balance`'s `balance_microusd` are
//! `bigint`, so they decode through [`aex_rds_data::Record::i64`]; the only
//! `numeric(38, 0)` column is a usage quantity, which the Data `API` renders as
//! a `stringValue` and [`numeric_u128`] parses exactly.

use aex_finance_domain::{Microusd, MicrousdDelta, MoneyError};
use aex_rds_data::{DecodeError, Record};

/// Data API maximum bytes in one row.
pub const DATA_API_MAX_ROW_BYTES: usize = 64 * 1024;
/// Data API maximum response bytes.
pub const DATA_API_MAX_RESULT_BYTES: usize = 1024 * 1024;

/// Re-states a transport decode failure in the finance vocabulary.
///
/// Only the two shapes finance has a specific rule about are renamed: a float in
/// a money column and a null in a required one. Everything else keeps the
/// transport's own classification rather than being flattened into a guess.
fn money_decode(error: DecodeError) -> RowDecodeError {
    match error {
        DecodeError::UnexpectedDoubleValue { .. } => RowDecodeError::FloatInMoneyColumn,
        DecodeError::UnexpectedNull { .. } => RowDecodeError::Null,
        other => RowDecodeError::Transport(other),
    }
}

/// Decodes a non-negative `bigint` money column.
///
/// # Errors
/// Rejects floats, null, every non-`bigint` transport shape, negatives and
/// business-bound overflow.
pub fn bigint_money(record: &Record<'_>, index: usize) -> Result<Microusd, RowDecodeError> {
    let raw = record.i64(index).map_err(money_decode)?;
    Microusd::new(raw).map_err(RowDecodeError::Money)
}

/// Decodes a signed `bigint` posting.
///
/// # Errors
/// Rejects floats, null, every non-`bigint` transport shape and business-bound
/// overflow.
pub fn bigint_delta(record: &Record<'_>, index: usize) -> Result<MicrousdDelta, RowDecodeError> {
    let raw = record.i64(index).map_err(money_decode)?;
    MicrousdDelta::new(raw).map_err(RowDecodeError::Money)
}

/// Decodes `numeric(38, 0)` exclusively from a canonical string.
///
/// # Errors
/// Rejects every non-string transport shape and non-canonical/overflowing text.
pub fn numeric_u128(record: &Record<'_>, index: usize) -> Result<u128, RowDecodeError> {
    let value = record.text(index).map_err(|error| match error {
        DecodeError::TypeMismatch { .. } => RowDecodeError::QuantityNotString,
        other => money_decode(other),
    })?;
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
    /// The transport refused the column for a reason finance has no rule about.
    #[error(transparent)]
    Transport(DecodeError),
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
