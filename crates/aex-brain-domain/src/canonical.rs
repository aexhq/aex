//! RFC 8785 JSON Canonicalization Scheme.
//!
//! One canonicalization rule holds workspace-wide, so no stream may introduce a second
//! canonicalizer or comparator. This module is that rule for Brain payloads until
//! `aex-wire` publishes it.
//!
//! Two deliberate strictnesses over bare RFC 8785:
//!
//! - a non-integer number is **rejected**, not formatted. Money is integer micro-USD
//!   (`OD-13`) and every quantity in the Brain is an integer, so a float in a canonical
//!   body is a defect upstream, and silently reformatting it would hide the defect while
//!   changing a hash;
//! - the encoder is bounded by depth and byte budget, so an adversarial document costs a
//!   typed error rather than a stack overflow.

use serde_json::{Map, Value};

/// The deepest structure a canonical Brain body may contain.
///
/// Journal payloads are shallow by construction; anything deeper is either a defect or an
/// attempt to exhaust the stack through a tool input.
pub const MAX_DEPTH: usize = 32;

/// The largest canonical encoding this module will produce.
///
/// This is the encoder's own adversarial bound, not the `DynamoDB` item ceiling. It sits at
/// the aggregate transaction ceiling because no single body can legitimately exceed what a
/// whole transaction carries, while staying strictly **above**
/// [`crate::commit::MAX_ITEM_BYTES`] so an over-large record is diagnosed by
/// [`crate::commit::EnvelopeViolation::ItemTooLarge`] — which names the item — rather than
/// collapsing into an undiagnosable [`CanonicalizeError::TooLarge`]. A validator that
/// cannot tell "this record is too big for one item" from "this document is hostile" is
/// useless to the caller that has to decide whether to page or to place the body in the
/// content authority.
pub const MAX_CANONICAL_BYTES: usize = 4 * 1_024 * 1_024;

/// Why a value could not be canonicalized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalizeError {
    /// A number was not an exact integer.
    #[error("non-integer number `{literal}` at `{path}`: canonical bodies carry integers only")]
    NonIntegerNumber {
        /// Where the number was found, as a JSON pointer.
        path: String,
        /// The offending literal.
        literal: String,
    },
    /// The document nested deeper than [`MAX_DEPTH`].
    #[error("depth {depth} exceeds the {MAX_DEPTH} permitted at `{path}`")]
    TooDeep {
        /// Where the limit was hit, as a JSON pointer.
        path: String,
        /// The depth reached.
        depth: usize,
    },
    /// The encoding grew past [`MAX_CANONICAL_BYTES`].
    #[error("canonical encoding exceeds {MAX_CANONICAL_BYTES} bytes")]
    TooLarge,
    /// The value could not be represented as JSON at all.
    #[error("value is not representable as JSON: {reason}")]
    NotJson {
        /// Why serialization failed.
        reason: String,
    },
}

/// Canonicalizes `value` to its RFC 8785 byte form.
///
/// # Errors
///
/// Returns [`CanonicalizeError`] when the value carries a non-integer number, nests deeper
/// than [`MAX_DEPTH`], or encodes to more than [`MAX_CANONICAL_BYTES`].
pub fn canonicalize(value: &Value) -> Result<Vec<u8>, CanonicalizeError> {
    let mut out = Vec::with_capacity(256);
    write_value(value, &mut out, 0, &mut String::new())?;
    if out.len() > MAX_CANONICAL_BYTES {
        return Err(CanonicalizeError::TooLarge);
    }
    Ok(out)
}

/// Canonicalizes a serializable payload.
///
/// # Errors
///
/// Returns [`CanonicalizeError::NotJson`] when the payload cannot be represented as JSON,
/// and otherwise every error [`canonicalize`] returns.
pub fn canonicalize_value<T>(payload: &T) -> Result<Vec<u8>, CanonicalizeError>
where
    T: serde::Serialize,
{
    let value = serde_json::to_value(payload).map_err(|error| CanonicalizeError::NotJson {
        reason: error.to_string(),
    })?;
    canonicalize(&value)
}

fn write_value(
    value: &Value,
    out: &mut Vec<u8>,
    depth: usize,
    path: &mut String,
) -> Result<(), CanonicalizeError> {
    if depth > MAX_DEPTH {
        return Err(CanonicalizeError::TooDeep {
            path: path.clone(),
            depth,
        });
    }
    if out.len() > MAX_CANONICAL_BYTES {
        return Err(CanonicalizeError::TooLarge);
    }
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(number) => write_number(number, out, path)?,
        Value::String(text) => write_string(text, out),
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                let mark = path.len();
                path.push('/');
                path.push_str(itoa(index).as_str());
                write_value(item, out, depth + 1, path)?;
                path.truncate(mark);
            }
            out.push(b']');
        }
        Value::Object(members) => write_object(members, out, depth, path)?,
    }
    Ok(())
}

fn write_object(
    members: &Map<String, Value>,
    out: &mut Vec<u8>,
    depth: usize,
    path: &mut String,
) -> Result<(), CanonicalizeError> {
    // RFC 8785 orders members by the UTF-16 code units of their names, which differs from
    // the UTF-8 byte order `serde_json`'s map already uses for names outside the BMP.
    let mut keyed: Vec<(Vec<u16>, &String, &Value)> = members
        .iter()
        .map(|(name, member)| (name.encode_utf16().collect(), name, member))
        .collect();
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    out.push(b'{');
    for (index, (_, name, member)) in keyed.iter().enumerate() {
        if index > 0 {
            out.push(b',');
        }
        write_string(name, out);
        out.push(b':');
        let mark = path.len();
        path.push('/');
        path.push_str(&name.replace('~', "~0").replace('/', "~1"));
        write_value(member, out, depth + 1, path)?;
        path.truncate(mark);
    }
    out.push(b'}');
    Ok(())
}

fn write_number(
    number: &serde_json::Number,
    out: &mut Vec<u8>,
    path: &str,
) -> Result<(), CanonicalizeError> {
    if let Some(value) = number.as_i64() {
        out.extend_from_slice(itoa_i64(value).as_bytes());
        return Ok(());
    }
    if let Some(value) = number.as_u64() {
        out.extend_from_slice(itoa(usize::try_from(value).unwrap_or(usize::MAX)).as_bytes());
        return Ok(());
    }
    Err(CanonicalizeError::NonIntegerNumber {
        path: path.to_owned(),
        literal: number.to_string(),
    })
}

fn write_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{08}' => out.extend_from_slice(b"\\b"),
            '\u{0C}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            control if (control as u32) < 0x20 => {
                let escaped = format!("\\u{:04x}", control as u32);
                out.extend_from_slice(escaped.as_bytes());
            }
            other => {
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}

fn itoa(value: usize) -> String {
    let mut text = String::with_capacity(20);
    let mut digits = [0_u8; 20];
    let mut index = 0;
    let mut remaining = value;
    loop {
        digits[index] = b'0' + u8::try_from(remaining % 10).unwrap_or(0);
        remaining /= 10;
        index += 1;
        if remaining == 0 {
            break;
        }
    }
    while index > 0 {
        index -= 1;
        text.push(char::from(digits[index]));
    }
    text
}

fn itoa_i64(value: i64) -> String {
    if value < 0 {
        let magnitude = value.unsigned_abs();
        let mut text = String::with_capacity(21);
        text.push('-');
        text.push_str(&itoa(usize::try_from(magnitude).unwrap_or(usize::MAX)));
        text
    } else {
        itoa(usize::try_from(value).unwrap_or(usize::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::{CanonicalizeError, MAX_DEPTH, canonicalize};
    use serde_json::json;

    #[test]
    fn members_are_ordered_by_utf16_code_units() {
        let value = json!({ "b": 1, "a": 2, "\u{10437}": 3, "\u{FB00}": 4 });
        let bytes = canonicalize(&value).expect("an integer document canonicalizes");
        let text = String::from_utf8(bytes).expect("canonical output is UTF-8");
        // U+10437 encodes to the surrogate D801 DC37, which sorts before U+FB00.
        assert_eq!(text, "{\"a\":2,\"b\":1,\"\u{10437}\":3,\"\u{FB00}\":4}");
    }

    #[test]
    fn the_same_document_written_two_ways_canonicalizes_identically() {
        let left = json!({ "outer": { "z": [1, 2], "a": "x" }, "n": -7 });
        let right = json!({ "n": -7, "outer": { "a": "x", "z": [1, 2] } });
        assert_eq!(
            canonicalize(&left).expect("left canonicalizes"),
            canonicalize(&right).expect("right canonicalizes")
        );
    }

    #[test]
    fn a_non_integer_number_is_rejected_rather_than_reformatted() {
        let error = canonicalize(&json!({ "cost": 1.5 })).expect_err("a float is rejected");
        assert!(
            matches!(error, CanonicalizeError::NonIntegerNumber { ref path, .. } if path == "/cost"),
            "{error:?}"
        );
    }

    #[test]
    fn control_characters_use_the_short_escapes_rfc_8785_requires() {
        let bytes =
            canonicalize(&json!({ "t": "a\nb\tc\u{0001}\"" })).expect("strings canonicalize");
        assert_eq!(
            String::from_utf8(bytes).expect("canonical output is UTF-8"),
            "{\"t\":\"a\\nb\\tc\\u0001\\\"\"}"
        );
    }

    #[test]
    fn a_document_deeper_than_the_bound_is_a_typed_error() {
        let mut value = json!(0);
        for _ in 0..=MAX_DEPTH {
            value = json!([value]);
        }
        let error = canonicalize(&value).expect_err("an over-deep document is rejected");
        assert!(
            matches!(error, CanonicalizeError::TooDeep { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn canonicalization_is_idempotent_through_a_reparse() {
        let value = json!({ "a": [1, { "b": "c" }], "d": null, "e": true });
        let once = canonicalize(&value).expect("first pass");
        let reparsed: serde_json::Value =
            serde_json::from_slice(&once).expect("canonical output reparses");
        assert_eq!(once, canonicalize(&reparsed).expect("second pass"));
    }
}
