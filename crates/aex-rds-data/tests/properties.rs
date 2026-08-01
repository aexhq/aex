//! Generated properties over the parameter codec and the failure classifier.

use aex_rds_data::error::{DataApiError, ExceptionKind, classify, sql_state};
use aex_rds_data::record::field_bytes;
use aex_rds_data::{Record, SqlValue};
use aws_sdk_rdsdata::types::Field;
use proptest::prelude::*;

/// Every `SqlValue` arm, as a generated value.
fn any_value() -> impl Strategy<Value = SqlValue> {
    prop_oneof![
        Just(SqlValue::Null),
        any::<bool>().prop_map(SqlValue::Bool),
        any::<i64>().prop_map(SqlValue::I64),
        ".{0,64}".prop_map(SqlValue::Text),
        any::<u128>().prop_map(|bits| SqlValue::Uuid(uuid::Uuid::from_u128(bits))),
        proptest::collection::vec(any::<u8>(), 0..64).prop_map(SqlValue::Bytes),
        any::<i64>().prop_map(SqlValue::TimestampMillis),
        proptest::collection::vec(".{0,16}", 0..8).prop_map(SqlValue::TextArray),
    ]
}

proptest! {
    /// No arm of the parameter codec can produce a `doubleValue`, which is the
    /// property that keeps floating point out of every schema rather than only
    /// out of the money schema.
    #[test]
    fn no_bound_parameter_is_ever_a_double(value in any_value()) {
        let parameter = value.to_parameter("p");
        prop_assert!(!matches!(parameter.value(), Some(Field::DoubleValue(_))));
    }

    /// A bound parameter always carries the name it was bound under.
    #[test]
    fn a_bound_parameter_keeps_its_name(value in any_value(), name in "[a-z_]{1,24}") {
        let parameter = value.to_parameter(&name);
        prop_assert_eq!(parameter.name(), Some(name.as_str()));
    }

    /// Epoch millis survive the bind/project round trip exactly, which is the
    /// whole reason timestamps are transported as integers.
    #[test]
    fn epoch_millis_round_trip_exactly(millis in -62_167_219_200_000_i64..=253_402_300_799_999_i64) {
        let fields = [Field::LongValue(millis)];
        let record = Record::new(&fields);
        let decoded = record.timestamp_millis(0).expect("a representable instant");
        let back = i64::try_from(decoded.unix_timestamp_nanos() / 1_000_000).expect("in range");
        prop_assert_eq!(back, millis);
    }

    /// Classification is total: every exception name and message produces a
    /// value, and an unmatched one is never transient.
    #[test]
    fn classification_is_total_and_conservative(message in ".{0,120}") {
        let error = classify(ExceptionKind::Unknown, &message);
        let fatal = matches!(error, DataApiError::Fatal { .. });
        prop_assert!(fatal);
        let transient = error.transient();
        prop_assert!(!transient);
    }

    /// A message with no `SQLState:` suffix never yields a state.
    #[test]
    fn a_message_without_a_suffix_has_no_sql_state(message in "[A-Za-z ]{0,80}") {
        prop_assert!(sql_state(&message).is_none());
    }

    /// Text fields are budgeted by their byte length, so the guard cannot be
    /// fooled by a multi-byte character.
    #[test]
    fn a_text_field_is_budgeted_by_bytes(text in ".{0,64}") {
        prop_assert_eq!(field_bytes(&Field::StringValue(text.clone())), text.len());
    }
}
