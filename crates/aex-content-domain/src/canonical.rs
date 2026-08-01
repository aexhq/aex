//! The single canonicalizer: RFC 8785 JCS with UTF-8 key ordering.
//!
//! Two rules are fixed workspace-wide and no stream may introduce a second of
//! either (D-23, and the §8a cross-stream requirement):
//!
//! * object members are emitted in ascending **UTF-8 byte order** of the key;
//! * every other production is RFC 8785 — no insignificant whitespace, minimal
//!   string escaping, and one canonical spelling per value.
//!
//! The accepted value profile is deliberately narrower than JSON. A request
//! body reaching this module has already been strictly validated, so numbers
//! are canonical decimal integers, `-0` is normalized to `0`, and a
//! floating-point or non-finite value is a hard error rather than a rounding
//! decision. That removes the whole ECMAScript shortest-round-trip number
//! problem from the hash and makes byte parity with the TypeScript SDK provable
//! rather than hopeful.

use core::fmt;
use std::collections::BTreeMap;

use crate::wire_pending::CanonicalBytes;

/// A JSON value in the accepted canonical profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalValue {
    /// JSON `null`.
    Null,
    /// JSON `true` / `false`.
    Bool(bool),
    /// A canonical decimal integer.
    Integer(i64),
    /// A Unicode string.
    String(String),
    /// An ordered array.
    Array(Vec<CanonicalValue>),
    /// An object, held sorted by UTF-8 byte order of the key.
    Object(BTreeMap<String, CanonicalValue>),
}

impl CanonicalValue {
    /// Parses a JSON document into the canonical profile.
    ///
    /// # Errors
    ///
    /// Returns [`CanonicalError`] for malformed JSON, a duplicate object key,
    /// a non-integer or out-of-range number, or trailing content.
    pub fn parse(json: &str) -> Result<Self, CanonicalError> {
        let mut parser = Parser {
            bytes: json.as_bytes(),
            at: 0,
            depth: 0,
        };
        parser.skip_whitespace();
        let value = parser.value()?;
        parser.skip_whitespace();
        if parser.at != parser.bytes.len() {
            return Err(CanonicalError::TrailingContent { at: parser.at });
        }
        Ok(value)
    }

    /// Writes the canonical byte form.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> CanonicalBytes {
        let mut out = Vec::new();
        self.write(&mut out);
        CanonicalBytes::from_canonicalizer(out)
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Null => out.extend_from_slice(b"null"),
            Self::Bool(true) => out.extend_from_slice(b"true"),
            Self::Bool(false) => out.extend_from_slice(b"false"),
            Self::Integer(value) => out.extend_from_slice(value.to_string().as_bytes()),
            Self::String(text) => write_string(text, out),
            Self::Array(items) => {
                out.push(b'[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    item.write(out);
                }
                out.push(b']');
            }
            Self::Object(members) => {
                out.push(b'{');
                // `BTreeMap<String, _>` orders by `str`'s `Ord`, which is
                // lexicographic over UTF-8 bytes - exactly the pinned rule.
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.push(b',');
                    }
                    write_string(key, out);
                    out.push(b':');
                    value.write(out);
                }
                out.push(b'}');
            }
        }
    }
}

/// Canonicalizes a JSON document in one step.
///
/// # Errors
///
/// Returns [`CanonicalError`] when the document is outside the accepted
/// profile.
pub fn canonicalize(json: &str) -> Result<CanonicalBytes, CanonicalError> {
    CanonicalValue::parse(json).map(|value| value.to_canonical_bytes())
}

/// RFC 8785 string production: minimal escaping, lower-case `\u00xx` for the
/// remaining control characters.
fn write_string(text: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            control if (control as u32) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", control as u32).as_bytes());
            }
            other => {
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Why a document was rejected by the canonicalizer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonicalError {
    /// The document ended while a value was still expected.
    #[error("unexpected end of document at byte {at}")]
    UnexpectedEnd {
        /// Byte offset where input ran out.
        at: usize,
    },
    /// An unexpected byte appeared.
    #[error("unexpected byte at {at}: expected {expected}")]
    Unexpected {
        /// Byte offset of the offending byte.
        at: usize,
        /// What the parser required there.
        expected: &'static str,
    },
    /// Content followed the top-level value.
    #[error("trailing content at byte {at}")]
    TrailingContent {
        /// Byte offset of the first trailing byte.
        at: usize,
    },
    /// A number was not a canonical decimal integer.
    #[error("number at byte {at} is not a canonical decimal integer")]
    NonIntegerNumber {
        /// Byte offset where the number started.
        at: usize,
    },
    /// An object declared the same key twice.
    #[error("duplicate object key `{key}`")]
    DuplicateKey {
        /// The repeated key.
        key: String,
    },
    /// A string contained an invalid escape or an unpaired surrogate.
    #[error("invalid string escape at byte {at}")]
    InvalidEscape {
        /// Byte offset of the offending escape.
        at: usize,
    },
    /// Nesting exceeded [`MAX_DEPTH`].
    #[error("nesting deeper than {MAX_DEPTH}")]
    TooDeep,
}

/// Maximum accepted nesting depth. A bounded depth makes the recursive parser
/// total on hostile input.
pub const MAX_DEPTH: u32 = 64;

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: u32,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while let Some(byte) = self.bytes.get(self.at) {
            if matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
                self.at += 1;
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Result<u8, CanonicalError> {
        self.bytes
            .get(self.at)
            .copied()
            .ok_or(CanonicalError::UnexpectedEnd { at: self.at })
    }

    fn expect(&mut self, byte: u8, expected: &'static str) -> Result<(), CanonicalError> {
        if self.peek()? == byte {
            self.at += 1;
            Ok(())
        } else {
            Err(CanonicalError::Unexpected {
                at: self.at,
                expected,
            })
        }
    }

    fn literal(&mut self, word: &'static [u8]) -> Result<(), CanonicalError> {
        if self.bytes.len() >= self.at + word.len()
            && &self.bytes[self.at..self.at + word.len()] == word
        {
            self.at += word.len();
            Ok(())
        } else {
            Err(CanonicalError::Unexpected {
                at: self.at,
                expected: "a JSON literal",
            })
        }
    }

    fn value(&mut self) -> Result<CanonicalValue, CanonicalError> {
        if self.depth >= MAX_DEPTH {
            return Err(CanonicalError::TooDeep);
        }
        match self.peek()? {
            b'n' => self.literal(b"null").map(|()| CanonicalValue::Null),
            b't' => self.literal(b"true").map(|()| CanonicalValue::Bool(true)),
            b'f' => self.literal(b"false").map(|()| CanonicalValue::Bool(false)),
            b'"' => self.string().map(CanonicalValue::String),
            b'[' => self.array(),
            b'{' => self.object(),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(CanonicalError::Unexpected {
                at: self.at,
                expected: "a JSON value",
            }),
        }
    }

    fn array(&mut self) -> Result<CanonicalValue, CanonicalError> {
        self.expect(b'[', "`[`")?;
        self.depth += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek()? == b']' {
            self.at += 1;
            self.depth -= 1;
            return Ok(CanonicalValue::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b']' => {
                    self.at += 1;
                    break;
                }
                _ => {
                    return Err(CanonicalError::Unexpected {
                        at: self.at,
                        expected: "`,` or `]`",
                    });
                }
            }
        }
        self.depth -= 1;
        Ok(CanonicalValue::Array(items))
    }

    fn object(&mut self) -> Result<CanonicalValue, CanonicalError> {
        self.expect(b'{', "`{`")?;
        self.depth += 1;
        let mut members: BTreeMap<String, CanonicalValue> = BTreeMap::new();
        self.skip_whitespace();
        if self.peek()? == b'}' {
            self.at += 1;
            self.depth -= 1;
            return Ok(CanonicalValue::Object(members));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':', "`:`")?;
            self.skip_whitespace();
            let value = self.value()?;
            if members.insert(key.clone(), value).is_some() {
                return Err(CanonicalError::DuplicateKey { key });
            }
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {
                    self.at += 1;
                    break;
                }
                _ => {
                    return Err(CanonicalError::Unexpected {
                        at: self.at,
                        expected: "`,` or `}`",
                    });
                }
            }
        }
        self.depth -= 1;
        Ok(CanonicalValue::Object(members))
    }

    fn number(&mut self) -> Result<CanonicalValue, CanonicalError> {
        let start = self.at;
        if self.peek()? == b'-' {
            self.at += 1;
        }
        let digits_start = self.at;
        while matches!(self.bytes.get(self.at), Some(byte) if byte.is_ascii_digit()) {
            self.at += 1;
        }
        if self.at == digits_start {
            return Err(CanonicalError::Unexpected {
                at: self.at,
                expected: "a decimal digit",
            });
        }
        // A fraction or exponent is outside the accepted profile: money and
        // every other quantity crossing this boundary is an integer.
        if matches!(self.bytes.get(self.at), Some(b'.' | b'e' | b'E')) {
            return Err(CanonicalError::NonIntegerNumber { at: start });
        }
        let text = core::str::from_utf8(&self.bytes[start..self.at])
            .map_err(|_| CanonicalError::NonIntegerNumber { at: start })?;
        // Leading zeros and `-0` are not canonical spellings.
        let unsigned = text.strip_prefix('-').unwrap_or(text);
        if (unsigned.len() > 1 && unsigned.starts_with('0')) || (text.starts_with('-') && unsigned == "0")
        {
            return Err(CanonicalError::NonIntegerNumber { at: start });
        }
        text.parse::<i64>()
            .map(CanonicalValue::Integer)
            .map_err(|_| CanonicalError::NonIntegerNumber { at: start })
    }

    fn string(&mut self) -> Result<String, CanonicalError> {
        self.expect(b'"', "`\"`")?;
        let mut out = String::new();
        loop {
            let byte = self.peek()?;
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    let escape_at = self.at;
                    self.at += 1;
                    let escape = self.peek()?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape(escape_at)?),
                        _ => return Err(CanonicalError::InvalidEscape { at: escape_at }),
                    }
                }
                control if control < 0x20 => {
                    return Err(CanonicalError::Unexpected {
                        at: self.at,
                        expected: "an escaped control character",
                    });
                }
                _ => {
                    let rest = core::str::from_utf8(&self.bytes[self.at..]).map_err(|_| {
                        CanonicalError::Unexpected {
                            at: self.at,
                            expected: "valid UTF-8",
                        }
                    })?;
                    let character = rest.chars().next().ok_or(CanonicalError::UnexpectedEnd {
                        at: self.at,
                    })?;
                    self.at += character.len_utf8();
                    out.push(character);
                }
            }
        }
    }

    fn hex4(&mut self, escape_at: usize) -> Result<u32, CanonicalError> {
        if self.at + 4 > self.bytes.len() {
            return Err(CanonicalError::UnexpectedEnd { at: self.at });
        }
        let mut value = 0_u32;
        for offset in 0..4 {
            let digit = char::from(self.bytes[self.at + offset])
                .to_digit(16)
                .ok_or(CanonicalError::InvalidEscape { at: escape_at })?;
            value = value * 16 + digit;
        }
        self.at += 4;
        Ok(value)
    }

    fn unicode_escape(&mut self, escape_at: usize) -> Result<char, CanonicalError> {
        let first = self.hex4(escape_at)?;
        let scalar = if (0xd800..0xdc00).contains(&first) {
            if self.bytes.get(self.at) != Some(&b'\\')
                || self.bytes.get(self.at + 1) != Some(&b'u')
            {
                return Err(CanonicalError::InvalidEscape { at: escape_at });
            }
            self.at += 2;
            let second = self.hex4(escape_at)?;
            if !(0xdc00..0xe000).contains(&second) {
                return Err(CanonicalError::InvalidEscape { at: escape_at });
            }
            0x1_0000 + ((first - 0xd800) << 10) + (second - 0xdc00)
        } else if (0xdc00..0xe000).contains(&first) {
            return Err(CanonicalError::InvalidEscape { at: escape_at });
        } else {
            first
        };
        char::from_u32(scalar).ok_or(CanonicalError::InvalidEscape { at: escape_at })
    }
}

impl fmt::Display for CanonicalValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.to_canonical_bytes();
        // Canonical output is UTF-8 by construction.
        f.write_str(core::str::from_utf8(bytes.as_bytes()).unwrap_or("<non-utf8>"))
    }
}

#[cfg(test)]
mod tests {
    use super::{canonicalize, CanonicalError, CanonicalValue};

    fn canonical(json: &str) -> String {
        String::from_utf8(canonicalize(json).expect("accepted").as_bytes().to_vec())
            .expect("utf-8")
    }

    #[test]
    fn key_order_and_whitespace_do_not_change_the_canonical_form() {
        let a = canonical(r#"{ "b" : 1 , "a" : [ 1 , 2 ] }"#);
        let b = canonical(r#"{"a":[1,2],"b":1}"#);
        assert_eq!(a, b);
        assert_eq!(a, r#"{"a":[1,2],"b":1}"#);
    }

    #[test]
    fn keys_sort_by_utf8_byte_order() {
        let out = canonical(r#"{"z":0,"é":0,"a":0,"Z":0}"#);
        assert_eq!(out, r#"{"Z":0,"a":0,"z":0,"é":0}"#);
    }

    #[test]
    fn non_integer_numbers_are_rejected() {
        assert!(matches!(
            CanonicalValue::parse("1.5"),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        assert!(matches!(
            CanonicalValue::parse("1e3"),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        assert!(matches!(
            CanonicalValue::parse("-0"),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        assert!(matches!(
            CanonicalValue::parse("01"),
            Err(CanonicalError::NonIntegerNumber { .. })
        ));
        assert_eq!(canonical("0"), "0");
        assert_eq!(canonical("-17"), "-17");
    }

    #[test]
    fn duplicate_keys_and_trailing_content_are_rejected() {
        assert!(matches!(
            CanonicalValue::parse(r#"{"a":1,"a":2}"#),
            Err(CanonicalError::DuplicateKey { .. })
        ));
        assert!(matches!(
            CanonicalValue::parse("1 2"),
            Err(CanonicalError::TrailingContent { .. })
        ));
    }

    #[test]
    fn string_escaping_is_minimal_and_surrogate_pairs_decode() {
        assert_eq!(canonical(r#""aA\/b""#), r#""aA/b""#);
        assert_eq!(canonical(r#"" ""#), r#"" ""#);
        assert_eq!(canonical(r#""😀""#), "\"\u{1f600}\"");
        assert!(matches!(
            CanonicalValue::parse(r#""\ud83d""#),
            Err(CanonicalError::InvalidEscape { .. })
        ));
        assert_eq!(canonical("\"tab\\there\""), r#""tab\there""#);
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = format!("{}1{}", "[".repeat(80), "]".repeat(80));
        assert!(matches!(
            CanonicalValue::parse(&deep),
            Err(CanonicalError::TooDeep)
        ));
    }
}
