//! RFC 8785 JSON Canonicalization Scheme and the digest form built on it.
//!
//! One canonicalization rule holds workspace-wide: RFC 8785 JCS over UTF-8
//! byte output, with object member names sorted by UTF-16 code unit. No stream
//! may introduce a second canonicalizer or comparator, so every digest in the
//! release surface is produced here.
//!
//! Floating-point numbers are rejected outright rather than serialized by the
//! ECMAScript `Number::toString` rule. Money never crosses the wire as a float
//! (OD-13) and no release document has a legitimate non-integer field, so the
//! shortest-round-trip printer would only ever serve to hide a mistake.

use std::fmt::Write as _;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{Exit, Result, ToolError};

/// Serialize a value to its canonical JSON byte string.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] when the value cannot be represented (a
/// non-integer number, or a type `serde_json` cannot express).
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    let json = serde_json::to_value(value).map_err(|err| {
        ToolError::single(
            Exit::ManifestInvalid,
            "canonical-json-unserializable",
            err.to_string(),
        )
    })?;
    let mut out = String::new();
    write_value(&json, &mut out)?;
    Ok(out)
}

/// Canonicalize an already-parsed JSON document.
///
/// # Errors
/// Returns [`Exit::ManifestInvalid`] when the document contains a
/// floating-point number.
pub fn canonicalize(value: &serde_json::Value) -> Result<String> {
    let mut out = String::new();
    write_value(value, &mut out)?;
    Ok(out)
}

/// `sha256:`-prefixed lowercase hex digest of arbitrary bytes.
#[must_use]
pub fn digest_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Bare lowercase hex `sha256` of arbitrary bytes, with no scheme prefix.
///
/// Used where an external format fixes the encoding, such as the ZIP central
/// directory or an S3 `checksum_sha256` comparison.
#[must_use]
pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Digest of a document, computed over its canonical bytes.
///
/// # Errors
/// Propagates canonicalization failure.
pub fn digest_document(value: &serde_json::Value) -> Result<String> {
    Ok(digest_bytes(canonicalize(value)?.as_bytes()))
}

/// Digest of a document with its own self-referential digest fields removed.
///
/// A document that carries its digest cannot include that field in the input,
/// or the digest would have to be a fixed point. Annotation fields are excluded
/// the same way so a changelog edit does not mint a new release identity.
///
/// # Errors
/// Propagates canonicalization failure.
pub fn digest_document_excluding(value: &serde_json::Value, excluded: &[&str]) -> Result<String> {
    let mut reduced = value.clone();
    if let Some(object) = reduced.as_object_mut() {
        for key in excluded {
            object.remove(*key);
        }
    }
    digest_document(&reduced)
}

/// The checked-in form: canonical content plus the trailing newline every file
/// in the repository ends with.
///
/// # Errors
/// Propagates canonicalization failure.
pub fn to_file_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    let mut text = to_string(value)?;
    text.push('\n');
    Ok(text.into_bytes())
}

fn write_value(value: &serde_json::Value, out: &mut String) -> Result<()> {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(true) => out.push_str("true"),
        serde_json::Value::Bool(false) => out.push_str("false"),
        serde_json::Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                let _ = write!(out, "{unsigned}");
            } else if let Some(signed) = number.as_i64() {
                let _ = write!(out, "{signed}");
            } else {
                return Err(ToolError::single(
                    Exit::ManifestInvalid,
                    "canonical-json-float",
                    format!(
                        "`{number}` is not an integer; release documents carry no \
                         floating-point value"
                    ),
                ));
            }
        }
        serde_json::Value::String(text) => write_string(text, out),
        serde_json::Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        serde_json::Value::Object(members) => {
            let mut keys: Vec<&String> = members.keys().collect();
            keys.sort_by(|left, right| utf16_cmp(left, right));
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(&members[*key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// RFC 8785 §3.2.2.2: member names sort by UTF-16 code unit, not by UTF-8 byte.
///
/// The two orders disagree for code points at or above `U+10000`, which sort
/// before `U+E000..U+FFFF` in UTF-16 and after them in UTF-8.
fn utf16_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let mut left_units = left.encode_utf16();
    let mut right_units = right.encode_utf16();
    loop {
        match (left_units.next(), right_units.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => {
                if a != b {
                    return a.cmp(&b);
                }
            }
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{9}' => out.push_str("\\t"),
            '\u{a}' => out.push_str("\\n"),
            '\u{c}' => out.push_str("\\f"),
            '\u{d}' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::{canonicalize, digest_document_excluding, to_string, utf16_cmp};
    use serde_json::json;

    #[test]
    fn object_members_are_sorted_and_whitespace_is_removed() {
        let value = json!({ "b": 1, "a": [2, 3], "c": { "z": true, "y": null } });
        assert_eq!(
            canonicalize(&value).unwrap(),
            r#"{"a":[2,3],"b":1,"c":{"y":null,"z":true}}"#
        );
    }

    #[test]
    fn digest_is_stable_under_key_permutation() {
        let first = json!({ "alpha": 1, "beta": { "x": "1", "y": "2" } });
        let second = json!({ "beta": { "y": "2", "x": "1" }, "alpha": 1 });
        assert_eq!(
            canonicalize(&first).unwrap(),
            canonicalize(&second).unwrap()
        );
    }

    #[test]
    fn control_characters_use_the_short_escapes_then_lowercase_hex() {
        // U+0001 has no short escape and is emitted as lowercase
        // \u0001; every other control character here uses the short
        // form RFC 8785 gives it.
        let value = json!({ "k": "a\u{1}b\nc\td\"e\\f" });
        assert_eq!(
            canonicalize(&value).unwrap(),
            "{\"k\":\"a\\u0001b\\nc\\td\\\"e\\\\f\"}"
        );
    }

    #[test]
    fn non_ascii_is_emitted_literally_as_utf8() {
        let value = json!({ "k": "é☃" });
        assert_eq!(canonicalize(&value).unwrap(), "{\"k\":\"é☃\"}");
    }

    #[test]
    fn floating_point_numbers_are_rejected() {
        let value = json!({ "amount": 1.5 });
        let err = canonicalize(&value).unwrap_err();
        assert_eq!(err.rules(), vec!["canonical-json-float"]);
    }

    #[test]
    fn negative_integers_round_trip() {
        assert_eq!(canonicalize(&json!({ "n": -42 })).unwrap(), r#"{"n":-42}"#);
    }

    #[test]
    fn utf16_order_places_astral_planes_after_the_bmp_private_use_area() {
        // "\u{E000}" is one UTF-16 unit; "\u{10000}" is a surrogate pair whose
        // lead unit 0xD800 sorts before 0xE000. UTF-8 byte order is the reverse.
        assert_eq!(utf16_cmp("\u{10000}", "\u{E000}"), std::cmp::Ordering::Less);
        assert!("\u{10000}" > "\u{E000}");
        let value = json!({ "\u{E000}": 1, "\u{10000}": 2 });
        let canonical = canonicalize(&value).unwrap();
        let astral = canonical.find('\u{10000}').unwrap();
        let bmp = canonical.find('\u{E000}').unwrap();
        assert!(astral < bmp, "canonical form was {canonical}");
    }

    #[test]
    fn self_digest_and_annotations_are_excluded_from_the_digest_input() {
        let with_fields = json!({
            "releaseId": "sha256:00",
            "annotations": { "changelog": "https://example.invalid/x" },
            "units": { "a": 1 }
        });
        let bare = json!({ "units": { "a": 1 } });
        assert_eq!(
            digest_document_excluding(&with_fields, &["releaseId", "annotations"]).unwrap(),
            digest_document_excluding(&bare, &["releaseId", "annotations"]).unwrap()
        );
    }

    #[test]
    fn serialize_path_agrees_with_the_value_path() {
        #[derive(serde::Serialize)]
        struct Doc {
            zulu: u8,
            alpha: &'static str,
        }
        let doc = Doc {
            zulu: 9,
            alpha: "a",
        };
        assert_eq!(to_string(&doc).unwrap(), r#"{"alpha":"a","zulu":9}"#);
    }
}
