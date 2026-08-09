//! Opaque continuation tokens.
//!
//! This crate owns the `cur_` envelope grammar and nothing else. Signing,
//! binding the token to an endpoint, principal scope, region, snapshot and
//! normalized filter, and expiry are the issuing authority's, because a contract
//! crate must never hold a secret or a crypto policy.

use std::fmt;

use crate::types::{ValueError, from_str_field};

/// An opaque signed continuation token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cursor(Box<str>);

impl Cursor {
    /// The fixed prefix. A cursor is not a resource id and never parses as one.
    pub const PREFIX: &'static str = "cur_";

    /// Largest accepted byte length, envelope included.
    pub const MAX_BYTES: usize = 4096;

    /// Parses the envelope.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the prefix is missing, the opaque envelope
    /// is malformed or longer than [`Cursor::MAX_BYTES`], or a segment contains
    /// a byte outside unpadded base64url. Issuers use either one segment or a
    /// `payload.tag` pair.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "Cursor";
        if text.len() > Self::MAX_BYTES {
            return Err(ValueError::Length {
                kind: KIND,
                min: Self::PREFIX.len() + 1,
                max: Self::MAX_BYTES,
                found: text.len(),
            });
        }
        let Some(body) = text.strip_prefix(Self::PREFIX) else {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "missing `cur_` prefix",
            });
        };
        if body.is_empty() {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "empty cursor body",
            });
        }
        let mut segments = body.split('.');
        let Some(first) = segments.next() else {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "empty cursor body",
            });
        };
        let second = segments.next();
        if first.is_empty() || second.is_some_and(str::is_empty) || segments.next().is_some() {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "malformed cursor segments",
            });
        }
        if let Some(offset) = body.bytes().position(|byte| {
            !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.')
        }) {
            return Err(ValueError::Character {
                kind: KIND,
                offset: Self::PREFIX.len() + offset,
            });
        }
        Ok(Self(text.into()))
    }

    /// The whole token, prefix included.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Cursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl serde::Serialize for Cursor {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for Cursor {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

#[cfg(test)]
mod tests {
    use super::Cursor;

    #[test]
    fn accepts_single_segment_and_payload_tag_envelopes() {
        for raw in ["cur_cGF5bG9hZA", "cur_cGF5bG9hZA.bWFj"] {
            assert_eq!(Cursor::parse(raw).expect("a cursor").as_str(), raw);
        }
    }

    #[test]
    fn rejects_missing_empty_or_extra_envelope_segments() {
        for raw in ["cur_", "cur_.tag", "cur_payload.", "cur_a.b.c"] {
            assert!(Cursor::parse(raw).is_err(), "{raw}");
        }
    }
}
