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
    /// Returns a [`ValueError`] when the prefix is missing, the body is empty or
    /// longer than [`Cursor::MAX_BYTES`], or a byte is outside unpadded
    /// base64url.
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
        if let Some(offset) = body
            .bytes()
            .position(|byte| !(byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
        {
            return Err(ValueError::Character {
                kind: KIND,
                offset: offset + Self::PREFIX.len(),
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
