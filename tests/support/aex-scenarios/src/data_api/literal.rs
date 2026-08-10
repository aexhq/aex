//! Reading a row back out of `PostgreSQL`'s own output functions.
//!
//! [`super::render`] projects `<alias>::text`, so one row arrives as one
//! composite literal — `(a,b,,"c,d")` — and the column list beside it names each
//! column's type. This module turns that pair into the positional
//! `Vec<aws_sdk_rdsdata::types::Field>` that `aex_rds_data::Record` decodes,
//! which is the whole reason the transport never touches the binary protocol:
//! `NUMERIC` keeps its exact decimal spelling, a `uuid` keeps its canonical
//! form and a `jsonb` document keeps its text, exactly as the Data `API`
//! delivers all three.
//!
//! # The grammar, and why it is written out rather than approximated
//!
//! `record_out` quotes a field when it is empty or contains `"`, `\`, `(`, `)`,
//! `,` or whitespace, and inside those quotes it doubles `"` and `\`. An
//! *unquoted empty* field is SQL `NULL`; a *quoted empty* field is the empty
//! string. Those two are one byte apart and mean opposite things, so the parser
//! distinguishes them rather than trimming.
//!
//! `array_out` is a second, different grammar: braces, `NULL` unquoted and
//! case-insensitive, elements quoted only when they need it, and backslash
//! escapes inside the quotes.

use aws_sdk_rdsdata::types::{ArrayValue, Field};
use aws_smithy_types::Blob;

/// Why a rendered row could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RowTextError {
    /// The row text was not a composite literal.
    #[error("row text `{text}` is not a parenthesised composite literal")]
    NotARow {
        /// What arrived.
        text: String,
    },
    /// A quoted field never closed.
    #[error("row text `{text}` has an unterminated quoted field")]
    Unterminated {
        /// What arrived.
        text: String,
    },
    /// The column metadata and the composite disagreed on arity.
    #[error("row text holds {actual} field(s) but the statement projects {expected} column(s)")]
    Arity {
        /// How many columns the statement projects.
        expected: usize,
        /// How many fields the composite held.
        actual: usize,
    },
    /// A `boolean` column was neither `t` nor `f`.
    #[error("column {index} of type `{postgres_type}` holds `{text}`, which is not a boolean")]
    NotABoolean {
        /// Zero-based column index.
        index: usize,
        /// The column's `PostgreSQL` type name.
        postgres_type: String,
        /// What arrived.
        text: String,
    },
    /// A numeric column did not parse as the type the column claims.
    #[error("column {index} of type `{postgres_type}` holds `{text}`, which is not a number")]
    NotANumber {
        /// Zero-based column index.
        index: usize,
        /// The column's `PostgreSQL` type name.
        postgres_type: String,
        /// What arrived.
        text: String,
    },
    /// A `bytea` column was not hex-encoded.
    #[error("column {index} holds `{text}`, which is not a `\\x`-prefixed bytea")]
    NotBytea {
        /// Zero-based column index.
        index: usize,
        /// What arrived.
        text: String,
    },
}

/// Which Data `API` field variant a `PostgreSQL` type projects as.
///
/// The mapping is the service's, not this module's invention: the Data `API`
/// answers `longValue` for the integer family, `booleanValue` for `boolean`,
/// `blobValue` for `bytea`, `arrayValue` for an array column, and
/// `stringValue` for everything else — including `uuid`, `NUMERIC`, `jsonb`,
/// every timestamp type and every enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    /// `boolean`.
    Bool,
    /// `smallint`, `integer`, `bigint`.
    Long,
    /// `bytea`.
    Blob,
    /// A one-dimensional array of text.
    TextArray,
    /// `real`, `double precision`.
    ///
    /// Deliberately preserved rather than refused here: producing
    /// `Field::DoubleValue` lets `aex_rds_data::Record` reject it with its own
    /// `UnexpectedDoubleValue`, which is the failure the platform's rule is
    /// written as.
    Double,
    /// Everything else.
    Text,
}

/// The projection for a `PostgreSQL` type name as `sqlx` reports it.
#[must_use]
pub fn projection_of(postgres_type: &str) -> Projection {
    match postgres_type.to_ascii_uppercase().as_str() {
        "BOOL" | "BOOLEAN" => Projection::Bool,
        "INT2" | "INT4" | "INT8" | "SMALLINT" | "INT" | "INTEGER" | "BIGINT" => Projection::Long,
        "BYTEA" => Projection::Blob,
        "FLOAT4" | "FLOAT8" | "REAL" | "DOUBLE PRECISION" => Projection::Double,
        other if other.ends_with("[]") => Projection::TextArray,
        _ => Projection::Text,
    }
}

/// Builds one field from a column's type and its rendered text.
///
/// # Errors
///
/// Returns [`RowTextError`] when the engine's own output does not parse as the
/// type the column claims, which is a defect in this module rather than in the
/// statement, and must therefore be loud.
pub fn field(
    index: usize,
    postgres_type: &str,
    text: Option<&str>,
) -> Result<Field, RowTextError> {
    let Some(text) = text else {
        return Ok(Field::IsNull(true));
    };
    Ok(match projection_of(postgres_type) {
        Projection::Bool => match text {
            "t" | "true" => Field::BooleanValue(true),
            "f" | "false" => Field::BooleanValue(false),
            other => {
                return Err(RowTextError::NotABoolean {
                    index,
                    postgres_type: postgres_type.to_owned(),
                    text: other.to_owned(),
                });
            }
        },
        Projection::Long => Field::LongValue(text.parse().map_err(|_| RowTextError::NotANumber {
            index,
            postgres_type: postgres_type.to_owned(),
            text: text.to_owned(),
        })?),
        Projection::Blob => {
            let hex = text
                .strip_prefix("\\x")
                .ok_or_else(|| RowTextError::NotBytea {
                    index,
                    text: text.to_owned(),
                })?;
            Field::BlobValue(Blob::new(hex::decode(hex).map_err(|_| {
                RowTextError::NotBytea {
                    index,
                    text: text.to_owned(),
                }
            })?))
        }
        Projection::TextArray => Field::ArrayValue(ArrayValue::StringValues(parse_array_literal(
            text,
        )?)),
        Projection::Double => {
            Field::DoubleValue(text.parse().map_err(|_| RowTextError::NotANumber {
                index,
                postgres_type: postgres_type.to_owned(),
                text: text.to_owned(),
            })?)
        }
        Projection::Text => Field::StringValue(text.to_owned()),
    })
}

/// Splits one composite literal into its fields.
///
/// `None` is SQL `NULL`; `Some(String::new())` is the empty string. The
/// difference is the presence of the quotes, and it is load-bearing.
///
/// # Errors
///
/// Returns [`RowTextError`] when the text is not a composite literal or a quoted
/// field never closes.
pub fn parse_row_literal(text: &str) -> Result<Vec<Option<String>>, RowTextError> {
    let inner = text
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .ok_or_else(|| RowTextError::NotARow {
            text: text.to_owned(),
        })?;
    // `()` is a one-column row holding NULL, not a zero-column row: `record_out`
    // writes an unquoted empty field per NULL and there is always at least one
    // column, because the renderer only wraps a statement that projects some.
    let mut fields = Vec::new();
    let bytes = inner.as_bytes();
    let mut index = 0_usize;
    let mut current = String::new();
    let mut quoted = false;
    let mut seen_quote = false;
    while index < bytes.len() {
        match bytes[index] {
            b'"' if !quoted => {
                quoted = true;
                seen_quote = true;
                index += 1;
            }
            b'"' if quoted => {
                if bytes.get(index + 1) == Some(&b'"') {
                    current.push('"');
                    index += 2;
                } else {
                    quoted = false;
                    index += 1;
                }
            }
            b'\\' if quoted => {
                let escaped = bytes.get(index + 1).ok_or_else(|| RowTextError::Unterminated {
                    text: text.to_owned(),
                })?;
                current.push(char::from(*escaped));
                index += 2;
            }
            b',' if !quoted => {
                fields.push((seen_quote || !current.is_empty()).then(|| current.clone()));
                current.clear();
                seen_quote = false;
                index += 1;
            }
            _ => {
                let character = inner[index..].chars().next().unwrap_or('\0');
                current.push(character);
                index += character.len_utf8();
            }
        }
    }
    if quoted {
        return Err(RowTextError::Unterminated {
            text: text.to_owned(),
        });
    }
    fields.push((seen_quote || !current.is_empty()).then_some(current));
    Ok(fields)
}

/// Splits one array literal into its elements.
///
/// # Errors
///
/// Returns [`RowTextError`] when the text is not an array literal or a quoted
/// element never closes.
pub fn parse_array_literal(text: &str) -> Result<Vec<Option<String>>, RowTextError> {
    let inner = text
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .ok_or_else(|| RowTextError::NotARow {
            text: text.to_owned(),
        })?;
    let mut elements = Vec::new();
    if inner.is_empty() {
        return Ok(elements);
    }
    let bytes = inner.as_bytes();
    let mut index = 0_usize;
    let mut current = String::new();
    let mut quoted = false;
    let mut seen_quote = false;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                quoted = !quoted;
                seen_quote = true;
                index += 1;
            }
            b'\\' if quoted => {
                let escaped = bytes.get(index + 1).ok_or_else(|| RowTextError::Unterminated {
                    text: text.to_owned(),
                })?;
                current.push(char::from(*escaped));
                index += 2;
            }
            b',' if !quoted => {
                elements.push(array_element(&current, seen_quote));
                current.clear();
                seen_quote = false;
                index += 1;
            }
            _ => {
                let character = inner[index..].chars().next().unwrap_or('\0');
                current.push(character);
                index += character.len_utf8();
            }
        }
    }
    if quoted {
        return Err(RowTextError::Unterminated {
            text: text.to_owned(),
        });
    }
    elements.push(array_element(&current, seen_quote));
    Ok(elements)
}

/// One array element, applying the unquoted-`NULL` rule.
fn array_element(raw: &str, quoted: bool) -> Option<String> {
    if !quoted && raw.eq_ignore_ascii_case("NULL") {
        return None;
    }
    Some(raw.to_owned())
}
