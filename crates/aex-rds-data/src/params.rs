//! Typed parameter binding.
//!
//! [`SqlValue`] is the whole vocabulary a statement may bind. There is no `f64`
//! arm and there never will be: a schema that needs a fraction stores `NUMERIC`
//! and binds [`SqlValue::NumericText`], which round-trips exactly.
//!
//! Timestamps are bound as epoch milliseconds and cast in SQL with
//! [`sql::MILLIS_TO_TIMESTAMPTZ`], and are projected back as `bigint`. That
//! removes every dependence on the session time zone, `DateStyle` and the Data
//! `API`'s own naive timestamp rendering.

use aws_sdk_rdsdata::types::{ArrayValue, Field, SqlParameter, TypeHint};
use aws_smithy_types::Blob;
use uuid::Uuid;

/// SQL fragments the repositories must use rather than spell themselves.
pub mod sql {
    /// Casts a bound epoch-millisecond `bigint` parameter to `timestamptz`.
    ///
    /// Used as `format!("{}", MILLIS_TO_TIMESTAMPTZ.replace("{}", ":issued_at_ms"))` is
    /// deliberately *not* how this is applied: repositories interpolate the
    /// parameter name into a `const &str` at authoring time, not at run time.
    pub const MILLIS_TO_TIMESTAMPTZ: &str =
        "(TIMESTAMPTZ 'epoch' + ({}) * INTERVAL '1 millisecond')";

    /// Projects a `timestamptz` column as epoch milliseconds.
    pub const TIMESTAMPTZ_TO_MILLIS: &str = "(EXTRACT(EPOCH FROM {})*1000)::bigint";
}

/// Every value this transport can bind.
///
/// `NumericText` carries the exact decimal spelling of a `NUMERIC`. Binding it
/// as a string is the only lossless option the Data `API` offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlValue {
    /// SQL `NULL`.
    Null,
    /// `boolean`.
    Bool(bool),
    /// `bigint`, `integer` and `smallint`.
    I64(i64),
    /// `text` and every domain over it.
    Text(String),
    /// `uuid`, bound with the `UUID` type hint.
    Uuid(Uuid),
    /// `bytea`.
    Bytes(Vec<u8>),
    /// `NUMERIC`, in its exact decimal spelling.
    NumericText(String),
    /// A `timestamptz`, as epoch milliseconds. The statement must apply
    /// [`sql::MILLIS_TO_TIMESTAMPTZ`] to the bound parameter.
    TimestampMillis(i64),
    /// `jsonb`, bound with the `JSON` type hint.
    Json(serde_json::Value),
    /// `text[]`.
    TextArray(Vec<String>),
}

impl SqlValue {
    /// Builds the Data `API` parameter for `name`.
    ///
    /// # Panics
    ///
    /// Never: `SqlParameter::builder` has no required field this function omits.
    #[must_use]
    pub fn to_parameter(&self, name: &str) -> SqlParameter {
        let builder = SqlParameter::builder().name(name);
        match self {
            Self::Null => builder.value(Field::IsNull(true)),
            Self::Bool(value) => builder.value(Field::BooleanValue(*value)),
            Self::I64(value) | Self::TimestampMillis(value) => {
                builder.value(Field::LongValue(*value))
            }
            Self::Text(value) => builder.value(Field::StringValue(value.clone())),
            Self::Uuid(value) => builder
                .value(Field::StringValue(value.to_string()))
                .type_hint(TypeHint::Uuid),
            Self::Bytes(value) => builder.value(Field::BlobValue(Blob::new(value.clone()))),
            Self::NumericText(value) => builder
                .value(Field::StringValue(value.clone()))
                .type_hint(TypeHint::Decimal),
            Self::Json(value) => builder
                .value(Field::StringValue(value.to_string()))
                .type_hint(TypeHint::Json),
            Self::TextArray(values) => builder.value(Field::ArrayValue(ArrayValue::StringValues(
                values.iter().cloned().map(Some).collect(),
            ))),
        }
        .build()
    }

    /// A stable, low-cardinality label for telemetry.
    ///
    /// Values are never recorded — only which arm was bound — because a
    /// parameter value is exactly what a credential looks like.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::Text(_) => "text",
            Self::Uuid(_) => "uuid",
            Self::Bytes(_) => "bytes",
            Self::NumericText(_) => "numeric",
            Self::TimestampMillis(_) => "timestamp_millis",
            Self::Json(_) => "json",
            Self::TextArray(_) => "text_array",
        }
    }
}

/// One statement and its bound parameters.
///
/// `sql` is a `&'a str` rather than a `String` so a repository can only supply a
/// borrowed constant. Every repository keeps its statements in a `sql` module of
/// `const &str`, which a source scan asserts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement<'a> {
    /// The statement text, always a compile-time constant at the call site.
    pub sql: &'a str,
    /// The named parameters, in binding order.
    pub params: Vec<(&'a str, SqlValue)>,
}

impl<'a> Statement<'a> {
    /// Builds a parameterless statement.
    #[must_use]
    pub const fn new(sql: &'a str) -> Self {
        Self {
            sql,
            params: Vec::new(),
        }
    }

    /// Builds a statement with parameters.
    #[must_use]
    pub fn with(sql: &'a str, params: Vec<(&'a str, SqlValue)>) -> Self {
        Self { sql, params }
    }

    /// Binds one more parameter.
    #[must_use]
    pub fn bind(mut self, name: &'a str, value: SqlValue) -> Self {
        self.params.push((name, value));
        self
    }

    /// The parameters as Data `API` values.
    #[must_use]
    pub fn parameters(&self) -> Vec<SqlParameter> {
        self.params
            .iter()
            .map(|(name, value)| value.to_parameter(name))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{SqlValue, Statement};
    use aws_sdk_rdsdata::types::{ArrayValue, Field, TypeHint};
    use uuid::Uuid;

    #[test]
    fn a_uuid_binds_as_a_string_with_the_uuid_hint() {
        let id = Uuid::from_u128(0x0192_3f2a_1c00_7000_8000_0000_0000_0001);
        let parameter = SqlValue::Uuid(id).to_parameter("id");
        assert_eq!(parameter.name(), Some("id"));
        assert_eq!(parameter.value(), Some(&Field::StringValue(id.to_string())),);
        assert_eq!(parameter.type_hint(), Some(&TypeHint::Uuid));
    }

    #[test]
    fn a_numeric_binds_as_a_string_with_the_decimal_hint() {
        let parameter = SqlValue::NumericText("12345.678901".to_owned()).to_parameter("amount");
        assert_eq!(
            parameter.value(),
            Some(&Field::StringValue("12345.678901".to_owned()))
        );
        assert_eq!(parameter.type_hint(), Some(&TypeHint::Decimal));
    }

    #[test]
    fn a_timestamp_binds_as_epoch_millis() {
        let parameter = SqlValue::TimestampMillis(1_767_225_600_123).to_parameter("issued_at_ms");
        assert_eq!(
            parameter.value(),
            Some(&Field::LongValue(1_767_225_600_123))
        );
        assert_eq!(parameter.type_hint(), None);
    }

    #[test]
    fn a_text_array_binds_as_string_values() {
        let parameter =
            SqlValue::TextArray(vec!["a:read".to_owned(), "b:write".to_owned()]).to_parameter("s");
        assert_eq!(
            parameter.value(),
            Some(&Field::ArrayValue(ArrayValue::StringValues(vec![
                Some("a:read".to_owned()),
                Some("b:write".to_owned())
            ])))
        );
    }

    #[test]
    fn a_null_binds_as_is_null() {
        let parameter = SqlValue::Null.to_parameter("maybe");
        assert_eq!(parameter.value(), Some(&Field::IsNull(true)));
    }

    #[test]
    fn json_binds_as_a_string_with_the_json_hint() {
        let parameter = SqlValue::Json(serde_json::json!({ "a": 1 })).to_parameter("detail");
        assert_eq!(
            parameter.value(),
            Some(&Field::StringValue("{\"a\":1}".to_owned()))
        );
        assert_eq!(parameter.type_hint(), Some(&TypeHint::Json));
    }

    #[test]
    fn a_statement_renders_its_parameters_in_binding_order() {
        let statement = Statement::new("SELECT 1")
            .bind("a", SqlValue::I64(1))
            .bind("b", SqlValue::Bool(true));
        let parameters = statement.parameters();
        assert_eq!(parameters.len(), 2);
        assert_eq!(parameters[0].name(), Some("a"));
        assert_eq!(parameters[1].name(), Some("b"));
    }

    #[test]
    fn every_arm_has_a_distinct_low_cardinality_label() {
        let labels = [
            SqlValue::Null.kind(),
            SqlValue::Bool(false).kind(),
            SqlValue::I64(0).kind(),
            SqlValue::Text(String::new()).kind(),
            SqlValue::Uuid(Uuid::nil()).kind(),
            SqlValue::Bytes(Vec::new()).kind(),
            SqlValue::NumericText(String::new()).kind(),
            SqlValue::TimestampMillis(0).kind(),
            SqlValue::Json(serde_json::Value::Null).kind(),
            SqlValue::TextArray(Vec::new()).kind(),
        ];
        let mut sorted = labels.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), labels.len());
    }
}
