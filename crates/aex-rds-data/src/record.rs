//! Row decoding.
//!
//! [`Record`] wraps one Data `API` record and exposes exactly the accessors this
//! platform's schemas need. There is deliberately **no** `f64` accessor: a
//! `doubleValue` in any column is [`DecodeError::UnexpectedDoubleValue`], so a
//! schema change that introduces floating point fails at the transport instead
//! of silently rounding.

use aws_sdk_rdsdata::types::{ArrayValue, Field};
use serde::de::DeserializeOwned;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::error::DecodeError;

/// One record of a result set.
#[derive(Debug, Clone, Copy)]
pub struct Record<'a>(&'a [Field]);

/// A type that can be built from one record.
pub trait Row: Sized {
    /// Decodes one record.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the record's arity or a column's Data `API`
    /// variant does not match what the row projects.
    fn from_record(record: &Record<'_>) -> Result<Self, DecodeError>;
}

/// How many bytes a field occupies for budget purposes.
#[must_use]
pub fn field_bytes(field: &Field) -> usize {
    match field {
        Field::StringValue(value) => value.len(),
        Field::BlobValue(value) => value.as_ref().len(),
        Field::LongValue(_) | Field::DoubleValue(_) => 8,
        Field::BooleanValue(_) => 1,
        Field::ArrayValue(value) => array_bytes(value),
        // `IsNull` costs nothing, and an unrecognised future variant is counted
        // as nothing rather than guessed at — the accessors reject it anyway,
        // so it can never reach a decoder under a wrong budget.
        _ => 0,
    }
}

/// How many bytes an array field occupies for budget purposes.
fn array_bytes(value: &ArrayValue) -> usize {
    match value {
        ArrayValue::StringValues(items) => items
            .iter()
            .map(|item| item.as_ref().map_or(0, String::len))
            .sum(),
        ArrayValue::LongValues(items) => items.len() * 8,
        ArrayValue::DoubleValues(items) => items.len() * 8,
        ArrayValue::BooleanValues(items) => items.len(),
        ArrayValue::ArrayValues(items) => items
            .iter()
            .map(|item| item.as_ref().map_or(0, array_bytes))
            .sum(),
        _ => 0,
    }
}

impl<'a> Record<'a> {
    /// Wraps a slice of fields.
    #[must_use]
    pub const fn new(fields: &'a [Field]) -> Self {
        Self(fields)
    }

    /// How many columns the record holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the record holds no columns at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Fails unless the record holds exactly `expected` columns.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::ArityMismatch`] when the counts differ.
    pub const fn expect_arity(&self, expected: usize) -> Result<(), DecodeError> {
        if self.0.len() == expected {
            Ok(())
        } else {
            Err(DecodeError::ArityMismatch {
                expected,
                actual: self.0.len(),
            })
        }
    }

    /// The raw field at `index`, rejecting `doubleValue` and a missing column.
    fn field(&self, index: usize) -> Result<&'a Field, DecodeError> {
        let field = self.0.get(index).ok_or(DecodeError::ArityMismatch {
            expected: index + 1,
            actual: self.0.len(),
        })?;
        if matches!(field, Field::DoubleValue(_)) {
            return Err(DecodeError::UnexpectedDoubleValue { index });
        }
        Ok(field)
    }

    /// Whether column `index` is SQL `NULL`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is absent or is a `doubleValue`.
    pub fn is_null(&self, index: usize) -> Result<bool, DecodeError> {
        Ok(matches!(self.field(index)?, Field::IsNull(true)))
    }

    /// Reads a `boolean`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is null, absent, a `doubleValue`
    /// or another Data `API` variant.
    pub fn bool(&self, index: usize) -> Result<bool, DecodeError> {
        match self.field(index)? {
            Field::BooleanValue(value) => Ok(*value),
            Field::IsNull(true) => Err(DecodeError::UnexpectedNull { index }),
            _ => Err(DecodeError::TypeMismatch {
                index,
                expected: "boolean",
            }),
        }
    }

    /// Reads a `bigint`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is null, absent, a `doubleValue`
    /// or another Data `API` variant.
    pub fn i64(&self, index: usize) -> Result<i64, DecodeError> {
        match self.field(index)? {
            Field::LongValue(value) => Ok(*value),
            Field::IsNull(true) => Err(DecodeError::UnexpectedNull { index }),
            _ => Err(DecodeError::TypeMismatch {
                index,
                expected: "bigint",
            }),
        }
    }

    /// Reads a `bigint` narrowed to `u16`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::Overflow`] when the value does not fit, plus every
    /// failure [`Record::i64`] reports.
    pub fn u16(&self, index: usize) -> Result<u16, DecodeError> {
        u16::try_from(self.i64(index)?).map_err(|_| DecodeError::Overflow { index })
    }

    /// Reads a `text`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is null, absent, a `doubleValue`
    /// or another Data `API` variant.
    pub fn text(&self, index: usize) -> Result<&'a str, DecodeError> {
        match self.field(index)? {
            Field::StringValue(value) => Ok(value.as_str()),
            Field::IsNull(true) => Err(DecodeError::UnexpectedNull { index }),
            _ => Err(DecodeError::TypeMismatch {
                index,
                expected: "text",
            }),
        }
    }

    /// Reads a `uuid`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BadUuid`] when the text is not a UUID, plus every
    /// failure [`Record::text`] reports.
    pub fn uuid(&self, index: usize) -> Result<Uuid, DecodeError> {
        Uuid::parse_str(self.text(index)?).map_err(|_| DecodeError::BadUuid { index })
    }

    /// Reads a `bytea`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is null, absent, a `doubleValue`
    /// or another Data `API` variant.
    pub fn bytes(&self, index: usize) -> Result<&'a [u8], DecodeError> {
        match self.field(index)? {
            Field::BlobValue(value) => Ok(value.as_ref()),
            Field::IsNull(true) => Err(DecodeError::UnexpectedNull { index }),
            _ => Err(DecodeError::TypeMismatch {
                index,
                expected: "bytea",
            }),
        }
    }

    /// Reads a fixed-width `bytea`, such as a 32-byte verifier.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::Overflow`] when the stored length is not `N`, plus
    /// every failure [`Record::bytes`] reports.
    pub fn fixed<const N: usize>(&self, index: usize) -> Result<[u8; N], DecodeError> {
        let raw = self.bytes(index)?;
        <[u8; N]>::try_from(raw).map_err(|_| DecodeError::Overflow { index })
    }

    /// Reads a `NUMERIC` as an integer scaled by `10^scale`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BadNumeric`] when the decimal spelling is not
    /// representable at `scale`, plus every failure [`Record::text`] reports.
    pub fn numeric_scaled(&self, index: usize, scale: u32) -> Result<i128, DecodeError> {
        let raw = self.text(index)?;
        parse_scaled(raw, scale).ok_or(DecodeError::BadNumeric { index })
    }

    /// Reads a `timestamptz` projected as epoch milliseconds.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BadTimestamp`] when the instant is outside the
    /// representable range, plus every failure [`Record::i64`] reports.
    pub fn timestamp_millis(&self, index: usize) -> Result<OffsetDateTime, DecodeError> {
        let millis = self.i64(index)?;
        OffsetDateTime::from_unix_timestamp_nanos(i128::from(millis) * 1_000_000)
            .map_err(|_| DecodeError::BadTimestamp { index })
    }

    /// Reads a `text[]`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError`] when the column is null, absent, a `doubleValue`,
    /// another Data `API` variant, or an array holding a null element.
    pub fn text_array(&self, index: usize) -> Result<Vec<&'a str>, DecodeError> {
        match self.field(index)? {
            Field::ArrayValue(ArrayValue::StringValues(items)) => items
                .iter()
                .map(|item| item.as_deref().ok_or(DecodeError::UnexpectedNull { index }))
                .collect(),
            Field::ArrayValue(ArrayValue::DoubleValues(_)) => {
                Err(DecodeError::UnexpectedDoubleValue { index })
            }
            Field::IsNull(true) => Err(DecodeError::UnexpectedNull { index }),
            _ => Err(DecodeError::TypeMismatch {
                index,
                expected: "text[]",
            }),
        }
    }

    /// Reads a `jsonb` column into `T`.
    ///
    /// # Errors
    ///
    /// Returns [`DecodeError::BadJson`] when the document does not match `T`,
    /// plus every failure [`Record::text`] reports.
    pub fn json<T: DeserializeOwned>(&self, index: usize) -> Result<T, DecodeError> {
        serde_json::from_str(self.text(index)?).map_err(|_| DecodeError::BadJson { index })
    }

    /// Reads a nullable column with `read`.
    ///
    /// # Errors
    ///
    /// Returns whatever `read` reports for a present value.
    pub fn opt<T>(
        &self,
        index: usize,
        read: impl FnOnce(&Self, usize) -> Result<T, DecodeError>,
    ) -> Result<Option<T>, DecodeError> {
        if self.is_null(index)? {
            return Ok(None);
        }
        read(self, index).map(Some)
    }
}

/// Parses an exact decimal spelling into an integer scaled by `10^scale`.
///
/// Rejects exponent notation and any fraction longer than `scale`, because both
/// would silently lose the exactness the `NUMERIC` column exists to keep.
fn parse_scaled(raw: &str, scale: u32) -> Option<i128> {
    let (negative, digits) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw.strip_prefix('+').unwrap_or(raw)),
    };
    if digits.is_empty()
        || !digits
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let mut parts = digits.split('.');
    let whole = parts.next()?;
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some() || whole.is_empty() {
        return None;
    }
    let scale = usize::try_from(scale).ok()?;
    if fraction.len() > scale {
        return None;
    }
    let mut value: i128 = whole.parse().ok()?;
    for _ in 0..scale {
        value = value.checked_mul(10)?;
    }
    let mut padded = fraction.to_owned();
    while padded.len() < scale {
        padded.push('0');
    }
    if !padded.is_empty() {
        value = value.checked_add(padded.parse::<i128>().ok()?)?;
    }
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::{Record, field_bytes, parse_scaled};
    use crate::error::DecodeError;
    use aws_sdk_rdsdata::types::{ArrayValue, Field};
    use aws_smithy_types::Blob;
    use uuid::Uuid;

    #[test]
    fn a_double_value_is_rejected_by_every_accessor() {
        let fields = [Field::DoubleValue(1.5)];
        let record = Record::new(&fields);
        let expected = DecodeError::UnexpectedDoubleValue { index: 0 };
        assert_eq!(record.bool(0).unwrap_err(), expected);
        assert_eq!(record.i64(0).unwrap_err(), expected);
        assert_eq!(record.text(0).unwrap_err(), expected);
        assert_eq!(record.bytes(0).unwrap_err(), expected);
        assert_eq!(record.uuid(0).unwrap_err(), expected);
        assert_eq!(record.numeric_scaled(0, 6).unwrap_err(), expected);
        assert_eq!(record.timestamp_millis(0).unwrap_err(), expected);
        assert_eq!(record.text_array(0).unwrap_err(), expected);
        assert_eq!(record.is_null(0).unwrap_err(), expected);
        assert_eq!(record.json::<serde_json::Value>(0).unwrap_err(), expected);
        assert_eq!(record.fixed::<32>(0).unwrap_err(), expected);
    }

    #[test]
    fn a_double_inside_an_array_is_rejected_too() {
        let fields = [Field::ArrayValue(ArrayValue::DoubleValues(vec![Some(1.0)]))];
        let record = Record::new(&fields);
        assert_eq!(
            record.text_array(0),
            Err(DecodeError::UnexpectedDoubleValue { index: 0 })
        );
    }

    #[test]
    fn a_null_in_a_non_optional_column_is_a_typed_failure() {
        let fields = [Field::IsNull(true)];
        let record = Record::new(&fields);
        assert_eq!(record.i64(0), Err(DecodeError::UnexpectedNull { index: 0 }));
        assert_eq!(record.opt(0, Record::i64), Ok(None));
    }

    #[test]
    fn a_missing_column_is_an_arity_mismatch() {
        let fields = [Field::LongValue(1)];
        let record = Record::new(&fields);
        assert_eq!(
            record.i64(3),
            Err(DecodeError::ArityMismatch {
                expected: 4,
                actual: 1
            })
        );
        assert_eq!(
            record.expect_arity(2),
            Err(DecodeError::ArityMismatch {
                expected: 2,
                actual: 1
            })
        );
    }

    #[test]
    fn epoch_millis_round_trip_through_the_timestamp_accessor() {
        let fields = [Field::LongValue(1_767_225_600_123)];
        let record = Record::new(&fields);
        let value = record.timestamp_millis(0).expect("a representable instant");
        assert_eq!(
            i64::try_from(value.unix_timestamp_nanos() / 1_000_000).expect("in range"),
            1_767_225_600_123
        );
    }

    #[test]
    fn a_fixed_width_blob_must_have_exactly_that_width() {
        let fields = [Field::BlobValue(Blob::new(vec![7_u8; 32]))];
        let record = Record::new(&fields);
        assert_eq!(record.fixed::<32>(0), Ok([7_u8; 32]));
        assert_eq!(
            record.fixed::<16>(0),
            Err(DecodeError::Overflow { index: 0 })
        );
    }

    #[test]
    fn a_uuid_column_decodes_and_rejects_a_non_uuid() {
        let id = Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001);
        let good = [Field::StringValue(id.to_string())];
        assert_eq!(Record::new(&good).uuid(0), Ok(id));
        let bad = [Field::StringValue("not-a-uuid".to_owned())];
        assert_eq!(
            Record::new(&bad).uuid(0),
            Err(DecodeError::BadUuid { index: 0 })
        );
    }

    #[test]
    fn numeric_text_decodes_exactly_at_the_requested_scale() {
        assert_eq!(parse_scaled("1", 6), Some(1_000_000));
        assert_eq!(parse_scaled("-0.000001", 6), Some(-1));
        assert_eq!(parse_scaled("12345.678901", 6), Some(12_345_678_901));
        assert_eq!(parse_scaled("0.0000001", 6), None);
        assert_eq!(parse_scaled("1e6", 6), None);
        assert_eq!(parse_scaled("", 6), None);
        assert_eq!(parse_scaled("1.2.3", 6), None);
    }

    #[test]
    fn a_text_array_with_a_null_element_is_a_typed_failure() {
        let fields = [Field::ArrayValue(ArrayValue::StringValues(vec![
            Some("a".to_owned()),
            None,
        ]))];
        assert_eq!(
            Record::new(&fields).text_array(0),
            Err(DecodeError::UnexpectedNull { index: 0 })
        );
    }

    #[test]
    fn field_bytes_counts_what_the_budget_cares_about() {
        assert_eq!(field_bytes(&Field::StringValue("abcd".to_owned())), 4);
        assert_eq!(field_bytes(&Field::BlobValue(Blob::new(vec![0_u8; 9]))), 9);
        assert_eq!(field_bytes(&Field::LongValue(1)), 8);
        assert_eq!(field_bytes(&Field::BooleanValue(true)), 1);
        assert_eq!(field_bytes(&Field::IsNull(true)), 0);
        assert_eq!(
            field_bytes(&Field::ArrayValue(ArrayValue::StringValues(vec![
                Some("ab".to_owned()),
                Some("cde".to_owned())
            ]))),
            5
        );
    }
}
