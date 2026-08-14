//! Identifier machinery: `UUIDv7` payloads, Crockford base32 rendering, the
//! [`PrefixedId`] trait every resource id implements, and the non-`UUID` wire
//! grammars (resource names, file paths, content hashes, W3C trace and span
//! ids, workspace API keys).
//!
//! The concrete id newtypes and the [`IdKind`] enum are generated from
//! `api/schemas/registries/ids.yaml` into `src/generated/ids.rs`; this module
//! owns everything they are built from.
//!
//! Two properties matter more than the encoding itself. A valid id of one kind
//! never parses as another, so a `SessionId` cannot be passed where a `MessageId`
//! is expected even through JSON. And the encoder never allocates: [`IdText`] is a
//! stack buffer, because id rendering happens on every log line and every
//! `DynamoDB` key.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use crate::generated::ids::{
    AccountId, AgentId, ApiKeyId, ApprovalId, BillingTransactionId, FileDownloadId, FileUploadId,
    GenerationId, IdKind, InvitationId, MeasurementId, MembershipId, MessageId, ObservationId,
    OperationId, OrganizationId, PaymentMethodId, ProviderCredentialId, SessionId, StatementId,
    ToolCallId, UploadId, UserId, WorkspaceId,
};
use crate::types::{Region, ValueError, from_str_field};

// ---------------------------------------------------------------------------
// Crockford base32
// ---------------------------------------------------------------------------

/// Crockford base32, lowercase, with `i`, `l`, `o` and `u` excluded.
const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";

/// Number of base32 characters a 128-bit payload renders as.
pub const SUFFIX_LEN: usize = 26;

/// Decodes one Crockford character, rejecting the excluded letters and every
/// uppercase spelling. The wire grammar is exact, so `I`/`L`/`O` are not folded
/// onto digits the way Crockford's lenient decoder would.
const fn decode_symbol(byte: u8) -> Option<u8> {
    let value = match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'h' => byte - b'a' + 10,
        b'j' | b'k' => byte - b'j' + 18,
        b'm' | b'n' => byte - b'm' + 20,
        b'p'..=b't' => byte - b'p' + 22,
        b'v'..=b'z' => byte - b'v' + 27,
        _ => return None,
    };
    Some(value)
}

// ---------------------------------------------------------------------------
// Uuid7
// ---------------------------------------------------------------------------

/// A validated `UUIDv7` payload.
///
/// Construction always checks the version and variant nibbles, so a `Uuid7`
/// value is time-ordered by definition and `Ord` on the bytes is `Ord` on the
/// embedded timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid7([u8; 16]);

impl Uuid7 {
    /// Wraps raw bytes.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError::NotUuidV7`] when the version nibble is not `7` or
    /// the variant bits are not `0b10`.
    pub const fn from_bytes(bytes: [u8; 16]) -> Result<Self, IdParseError> {
        if bytes[6] >> 4 != 0x7 || bytes[8] >> 6 != 0b10 {
            return Err(IdParseError::NotUuidV7);
        }
        Ok(Self(bytes))
    }

    /// The raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// The embedded 48-bit Unix millisecond timestamp.
    #[must_use]
    pub const fn unix_millis(&self) -> u64 {
        let bytes = &self.0;
        ((bytes[0] as u64) << 40)
            | ((bytes[1] as u64) << 32)
            | ((bytes[2] as u64) << 24)
            | ((bytes[3] as u64) << 16)
            | ((bytes[4] as u64) << 8)
            | (bytes[5] as u64)
    }

    /// Builds a `UUIDv7` from an explicit timestamp and entropy.
    ///
    /// There is no `Uuid7::new()`: a contract crate reads no clock and no RNG,
    /// so both inputs are the caller's.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "big-endian byte extraction; truncation is the operation"
    )]
    pub const fn compose(unix_millis: u64, entropy: [u8; 10]) -> Self {
        let mut bytes = [0u8; 16];
        bytes[0] = (unix_millis >> 40) as u8;
        bytes[1] = (unix_millis >> 32) as u8;
        bytes[2] = (unix_millis >> 24) as u8;
        bytes[3] = (unix_millis >> 16) as u8;
        bytes[4] = (unix_millis >> 8) as u8;
        bytes[5] = unix_millis as u8;
        bytes[6] = 0x70 | (entropy[0] & 0x0f);
        bytes[7] = entropy[1];
        bytes[8] = 0x80 | (entropy[2] & 0x3f);
        bytes[9] = entropy[3];
        bytes[10] = entropy[4];
        bytes[11] = entropy[5];
        bytes[12] = entropy[6];
        bytes[13] = entropy[7];
        bytes[14] = entropy[8];
        bytes[15] = entropy[9];
        Self(bytes)
    }

    /// Renders the 26-character Crockford base32 suffix.
    #[must_use]
    pub const fn encode_suffix(&self) -> [u8; SUFFIX_LEN] {
        let value = u128::from_be_bytes(self.0);
        let mut out = [0u8; SUFFIX_LEN];
        // The first character carries the top three bits; the remaining 25 carry
        // five each, which is exactly 128 bits.
        out[0] = ALPHABET[((value >> 125) & 0x07) as usize];
        let mut index = 1;
        while index < SUFFIX_LEN {
            let shift = 125 - (index * 5);
            out[index] = ALPHABET[((value >> shift) & 0x1f) as usize];
            index += 1;
        }
        out
    }

    /// Parses a 26-character Crockford base32 suffix.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError::MalformedSuffix`] for a wrong length, a character
    /// outside the alphabet, or a leading character that would overflow 128
    /// bits, and [`IdParseError::NotUuidV7`] when the decoded payload is not a
    /// `UUIDv7`.
    pub const fn decode_suffix(suffix: &[u8]) -> Result<Self, IdParseError> {
        if suffix.len() != SUFFIX_LEN {
            return Err(IdParseError::MalformedSuffix);
        }
        let mut value: u128 = 0;
        let mut index = 0;
        while index < SUFFIX_LEN {
            let Some(symbol) = decode_symbol(suffix[index]) else {
                return Err(IdParseError::MalformedSuffix);
            };
            if index == 0 {
                if symbol > 0x07 {
                    return Err(IdParseError::MalformedSuffix);
                }
                value = symbol as u128;
            } else {
                value = (value << 5) | (symbol as u128);
            }
            index += 1;
        }
        Self::from_bytes(value.to_be_bytes())
    }
}

impl fmt::Display for Uuid7 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let suffix = self.encode_suffix();
        formatter.write_str(std::str::from_utf8(&suffix).unwrap_or_default())
    }
}

impl Serialize for Uuid7 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let suffix = self.encode_suffix();
        serializer.serialize_str(std::str::from_utf8(&suffix).unwrap_or_default())
    }
}

impl<'de> Deserialize<'de> for Uuid7 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        let text = <std::borrow::Cow<'de, str> as Deserialize<'de>>::deserialize(deserializer)?;
        Self::decode_suffix(text.as_bytes()).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// IdText and IdParseError
// ---------------------------------------------------------------------------

/// A rendered identifier held in a stack buffer.
///
/// The longest prefix in the registry is four characters, so 32 bytes covers
/// every id without allocating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdText {
    /// Bytes used in `buf`.
    len: u8,
    /// The rendered text, ASCII only.
    buf: [u8; 32],
}

impl IdText {
    /// Builds the text for `prefix` and `suffix`.
    ///
    /// # Panics
    ///
    /// Panics when `prefix` is longer than five bytes, which the id registry
    /// forbids and a test asserts for every registered kind.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a prefix is at most five bytes and a suffix is 26, so the sum fits u8"
    )]
    pub const fn new(prefix: &str, suffix: &[u8; SUFFIX_LEN]) -> Self {
        let prefix = prefix.as_bytes();
        assert!(prefix.len() <= 5, "an id prefix is at most five bytes");
        let mut buf = [0u8; 32];
        let mut index = 0;
        while index < prefix.len() {
            buf[index] = prefix[index];
            index += 1;
        }
        buf[index] = b'_';
        index += 1;
        let mut position = 0;
        while position < SUFFIX_LEN {
            buf[index + position] = suffix[position];
            position += 1;
        }
        Self {
            len: (index + SUFFIX_LEN) as u8,
            buf,
        }
    }

    /// The rendered text.
    ///
    /// # Panics
    ///
    /// Never: the buffer only ever holds ASCII written by [`IdText::new`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len as usize]).expect("IdText holds ASCII")
    }
}

impl fmt::Display for IdText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an identifier failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdParseError {
    /// The text carried a different resource prefix.
    #[error("expected prefix `{expected}_`, found `{found}`")]
    WrongPrefix {
        /// The prefix the target type requires, without the underscore.
        expected: &'static str,
        /// What the value actually started with, truncated at the underscore.
        found: Box<str>,
    },
    /// The suffix was not 26 Crockford base32 characters.
    #[error("suffix must be 26 characters of [0-9a-hjkmnp-tv-z]")]
    MalformedSuffix,
    /// The decoded payload was not a `UUIDv7`.
    #[error("payload is not a UUIDv7 (version/variant nibbles)")]
    NotUuidV7,
}

// ---------------------------------------------------------------------------
// PrefixedId
// ---------------------------------------------------------------------------

/// The behaviour every AEX resource identifier shares.
pub trait PrefixedId:
    Copy
    + Ord
    + core::hash::Hash
    + fmt::Debug
    + fmt::Display
    + FromStr<Err = IdParseError>
    + Serialize
    + serde::de::DeserializeOwned
    + Sized
{
    /// Which resource this identifier names.
    const KIND: IdKind;

    /// Wraps a payload.
    fn from_uuid7(value: Uuid7) -> Self;

    /// The payload.
    fn uuid7(&self) -> Uuid7;

    /// Renders the identifier without allocating.
    fn encode(&self) -> IdText {
        IdText::new(Self::KIND.prefix(), &self.uuid7().encode_suffix())
    }

    /// Parses the identifier, rejecting every other kind's prefix.
    ///
    /// # Errors
    ///
    /// Returns [`IdParseError`] for a wrong prefix, a malformed suffix, or a
    /// payload that is not a `UUIDv7`.
    fn parse(text: &str) -> Result<Self, IdParseError> {
        let expected = Self::KIND.prefix();
        let Some(suffix) = text
            .strip_prefix(expected)
            .and_then(|rest| rest.strip_prefix('_'))
        else {
            let found = text.split_once('_').map_or(text, |(head, _)| head);
            return Err(IdParseError::WrongPrefix {
                expected,
                found: found.into(),
            });
        };
        Uuid7::decode_suffix(suffix.as_bytes()).map(Self::from_uuid7)
    }
}

/// Declares one identifier newtype. Invoked once per row of the id registry by
/// the generated `src/generated/ids.rs`.
macro_rules! prefixed_id {
    ($(#[$meta:meta])* $name:ident, $kind:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name($crate::ids::Uuid7);

        impl $crate::ids::PrefixedId for $name {
            const KIND: $crate::ids::IdKind = $crate::ids::IdKind::$kind;

            fn from_uuid7(value: $crate::ids::Uuid7) -> Self {
                Self(value)
            }

            fn uuid7(&self) -> $crate::ids::Uuid7 {
                self.0
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(<Self as $crate::ids::PrefixedId>::encode(self).as_str())
            }
        }

        impl ::core::fmt::Debug for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(formatter, concat!(stringify!($name), "({})"), self)
            }
        }

        impl ::core::str::FromStr for $name {
            type Err = $crate::ids::IdParseError;

            fn from_str(text: &str) -> ::core::result::Result<Self, Self::Err> {
                <Self as $crate::ids::PrefixedId>::parse(text)
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(
                &self,
                serializer: S,
            ) -> ::core::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(<Self as $crate::ids::PrefixedId>::encode(self).as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(
                deserializer: D,
            ) -> ::core::result::Result<Self, D::Error> {
                use ::serde::de::Error as _;
                let text = <::std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                <Self as $crate::ids::PrefixedId>::parse(text.as_ref()).map_err(D::Error::custom)
            }
        }
    };
}

pub(crate) use prefixed_id;

// ---------------------------------------------------------------------------
// Non-UUID wire grammars
// ---------------------------------------------------------------------------

/// The exact, case-sensitive grammar of a registered resource or secret name:
/// `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName(Box<str>);

impl ResourceName {
    /// The grammar, as published in the id registry.
    pub const PATTERN: &'static str = "^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$";

    /// Parses a resource name.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the name is empty, longer than 128 bytes,
    /// starts with a non-alphanumeric character, or contains a character outside
    /// `[A-Za-z0-9._-]`.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "ResourceName";
        if text.is_empty() || text.len() > 128 {
            return Err(ValueError::Length {
                kind: KIND,
                min: 1,
                max: 128,
                found: text.len(),
            });
        }
        let bytes = text.as_bytes();
        if !bytes[0].is_ascii_alphanumeric() {
            return Err(ValueError::Character {
                kind: KIND,
                offset: 0,
            });
        }
        if let Some(offset) = bytes
            .iter()
            .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')))
        {
            return Err(ValueError::Character { kind: KIND, offset });
        }
        Ok(Self(text.into()))
    }

    /// The name as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A normalized absolute POSIX path identifying a file entry.
///
/// Normalization is enforced, not applied: a path containing `.`, `..`, an
/// empty segment or a trailing slash is rejected rather than rewritten, because
/// a rewritten path would make two different requests share one identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FilePath(Box<str>);

impl FilePath {
    /// Largest accepted byte length.
    pub const MAX_BYTES: usize = 4096;

    /// Parses a normalized absolute POSIX path.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the path is empty, too long, relative,
    /// contains a NUL or control byte, or contains a `.`, `..`, empty or
    /// trailing segment.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "FilePath";
        if text.is_empty() || text.len() > Self::MAX_BYTES {
            return Err(ValueError::Length {
                kind: KIND,
                min: 1,
                max: Self::MAX_BYTES,
                found: text.len(),
            });
        }
        if let Some(offset) = text.bytes().position(|byte| byte < 0x20 || byte == 0x7f) {
            return Err(ValueError::Character { kind: KIND, offset });
        }
        if !text.starts_with('/') {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "path must be absolute",
            });
        }
        if text == "/" {
            return Ok(Self(text.into()));
        }
        for segment in text[1..].split('/') {
            if segment.is_empty() {
                return Err(ValueError::Grammar {
                    kind: KIND,
                    reason: "path has an empty or trailing segment",
                });
            }
            if segment == "." || segment == ".." {
                return Err(ValueError::Grammar {
                    kind: KIND,
                    reason: "path is not normalized (`.` or `..` segment)",
                });
            }
        }
        Ok(Self(text.into()))
    }

    /// The path as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A SHA-256 content hash, rendered `sha256:<64 lowercase hex>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Wraps a raw digest.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw digest.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hashes `bytes` with SHA-256.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Renders the wire spelling.
    #[must_use]
    pub fn to_wire(&self) -> String {
        let mut out = String::with_capacity(71);
        out.push_str("sha256:");
        for byte in self.0 {
            out.push(char::from(hex_digit(byte >> 4)));
            out.push(char::from(hex_digit(byte & 0x0f)));
        }
        out
    }

    /// Parses the wire spelling.
    ///
    /// # Errors
    ///
    /// Returns a [`ValueError`] when the prefix is missing, the length is wrong,
    /// or a digit is not lowercase hexadecimal.
    pub fn parse(text: &str) -> Result<Self, ValueError> {
        const KIND: &str = "ContentHash";
        let Some(hex) = text.strip_prefix("sha256:") else {
            return Err(ValueError::Grammar {
                kind: KIND,
                reason: "missing `sha256:` prefix",
            });
        };
        parse_lower_hex::<32>(KIND, hex).map(Self)
    }
}

/// A W3C trace identifier: 16 bytes rendered as 32 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TraceId([u8; 16]);

/// A W3C span identifier: 8 bytes rendered as 16 lowercase hex characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpanId([u8; 8]);

/// Declares the shared hexadecimal wire codec for a W3C identifier.
macro_rules! hex_id {
    ($name:ident, $len:literal, $kind:literal) => {
        impl $name {
            /// Wraps raw bytes.
            ///
            /// # Errors
            ///
            /// Returns a [`ValueError`] when every byte is zero, which W3C
            /// reserves as the invalid identifier.
            pub const fn from_bytes(bytes: [u8; $len]) -> Result<Self, ValueError> {
                let mut index = 0;
                while index < $len {
                    if bytes[index] != 0 {
                        return Ok(Self(bytes));
                    }
                    index += 1;
                }
                Err(ValueError::Grammar {
                    kind: $kind,
                    reason: "the all-zero identifier is invalid",
                })
            }

            /// The raw bytes.
            #[must_use]
            pub const fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            /// Renders the lowercase hexadecimal wire spelling.
            #[must_use]
            pub fn to_wire(&self) -> String {
                let mut out = String::with_capacity($len * 2);
                for byte in self.0 {
                    out.push(char::from(hex_digit(byte >> 4)));
                    out.push(char::from(hex_digit(byte & 0x0f)));
                }
                out
            }

            /// Parses the lowercase hexadecimal wire spelling.
            ///
            /// # Errors
            ///
            /// Returns a [`ValueError`] for a wrong length, an uppercase or
            /// non-hexadecimal digit, or the all-zero identifier.
            pub fn parse(text: &str) -> Result<Self, ValueError> {
                Self::from_bytes(parse_lower_hex::<$len>($kind, text)?)
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                formatter.write_str(&self.to_wire())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.to_wire())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                from_str_field(deserializer, Self::parse)
            }
        }
    };
}

hex_id!(TraceId, 16, "TraceId");
hex_id!(SpanId, 8, "SpanId");

/// The lowercase hexadecimal digit for a nibble.
const fn hex_digit(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + nibble - 10
    }
}

/// Parses exactly `N` bytes of lowercase hexadecimal.
fn parse_lower_hex<const N: usize>(kind: &'static str, text: &str) -> Result<[u8; N], ValueError> {
    if text.len() != N * 2 {
        return Err(ValueError::Length {
            kind,
            min: N * 2,
            max: N * 2,
            found: text.len(),
        });
    }
    let bytes = text.as_bytes();
    let mut out = [0u8; N];
    for (index, slot) in out.iter_mut().enumerate() {
        let high = decode_hex(kind, bytes[index * 2], index * 2)?;
        let low = decode_hex(kind, bytes[index * 2 + 1], index * 2 + 1)?;
        *slot = (high << 4) | low;
    }
    Ok(out)
}

/// Decodes one lowercase hexadecimal digit.
const fn decode_hex(kind: &'static str, byte: u8, offset: usize) -> Result<u8, ValueError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(ValueError::Character { kind, offset }),
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_wire())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_wire())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for ResourceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ResourceName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

impl fmt::Display for FilePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for FilePath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for FilePath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        from_str_field(deserializer, Self::parse)
    }
}

// ---------------------------------------------------------------------------
// WorkspaceApiKey
// ---------------------------------------------------------------------------

/// Why a workspace API key failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiKeyParseError {
    /// The value did not have the published shape.
    #[error("expected `aex_wk_<region>_<workspace-suffix>_<key-suffix>_<secret>`")]
    Shape,
    /// The region code was not one of the five published codes.
    #[error("unknown region code `{found}`")]
    UnknownRegion {
        /// The offending code.
        found: Box<str>,
    },
    /// The embedded workspace-id suffix was not a `UUIDv7` Crockford suffix.
    #[error("workspace-id suffix is invalid: {0}")]
    WorkspaceId(IdParseError),
    /// The embedded key-id suffix was not a `UUIDv7` Crockford suffix.
    #[error("key-id suffix is invalid: {0}")]
    KeyId(IdParseError),
    /// The secret was not 43 characters of unpadded base64url.
    #[error("secret must be 43 characters of unpadded base64url")]
    Secret,
}

/// A parsed one-time workspace API key.
///
/// The type deliberately has no `Display` and no `Serialize`, and its `Debug`
/// redacts the secret. Auditing every log site is not a control; making the
/// secret unrenderable is.
///
/// Both public identifiers it carries — the workspace and the key — are named
/// by the token so a regional endpoint can address every row it must read
/// before it has spoken to any central authority. Neither is a capability: the
/// 32 secret bytes are still the only thing that authenticates.
pub struct WorkspaceApiKey {
    /// Which regional endpoint the key belongs to.
    region: Region,
    /// The workspace the key authorizes.
    workspace_id: WorkspaceId,
    /// The key metadata identifier the verifier is looked up by.
    key_id: ApiKeyId,
    /// The 32 CSPRNG secret bytes, zeroized on drop.
    secret: zeroize::Zeroizing<[u8; 32]>,
}

impl WorkspaceApiKey {
    /// Parses
    /// `aex_wk_<region-code>_<26-char workspace suffix>_<26-char key suffix>_<43-char secret>`.
    ///
    /// # Errors
    ///
    /// Returns [`ApiKeyParseError`] for a wrong shape, an unknown region code,
    /// an invalid workspace- or key-id suffix, or a secret that is not 43
    /// characters of unpadded base64url.
    pub fn parse(text: &str) -> Result<Self, ApiKeyParseError> {
        let rest = text
            .strip_prefix("aex_wk_")
            .ok_or(ApiKeyParseError::Shape)?;
        let (region, rest) = rest.split_once('_').ok_or(ApiKeyParseError::Shape)?;
        let region = Region::from_code(region).ok_or_else(|| ApiKeyParseError::UnknownRegion {
            found: region.into(),
        })?;
        let (workspace_suffix, rest) = split_suffix(rest)?;
        let (key_suffix, secret) = split_suffix(rest)?;
        let workspace_id = WorkspaceId::from_uuid7(
            Uuid7::decode_suffix(workspace_suffix.as_bytes())
                .map_err(ApiKeyParseError::WorkspaceId)?,
        );
        let key_id = ApiKeyId::from_uuid7(
            Uuid7::decode_suffix(key_suffix.as_bytes()).map_err(ApiKeyParseError::KeyId)?,
        );
        let secret = decode_base64url_32(secret).ok_or(ApiKeyParseError::Secret)?;
        Ok(Self {
            region,
            workspace_id,
            key_id,
            secret: zeroize::Zeroizing::new(secret),
        })
    }

    /// The regional endpoint the key is pinned to.
    #[must_use]
    pub const fn region(&self) -> Region {
        self.region
    }

    /// The workspace the key authorizes.
    #[must_use]
    pub const fn workspace_id(&self) -> WorkspaceId {
        self.workspace_id
    }

    /// The key metadata identifier.
    #[must_use]
    pub const fn key_id(&self) -> ApiKeyId {
        self.key_id
    }

    /// The raw secret bytes.
    #[must_use]
    pub fn secret(&self) -> &[u8; 32] {
        &self.secret
    }
}

/// Takes one fixed-width id suffix and its separator from the head of `text`.
///
/// Canonical base64url legitimately contains `_`, so a searching split would let
/// a secret's own separator be read as a segment boundary. Every boundary after
/// the region is therefore taken at a fixed offset.
fn split_suffix(text: &str) -> Result<(&str, &str), ApiKeyParseError> {
    let head = text.get(..SUFFIX_LEN).ok_or(ApiKeyParseError::Shape)?;
    if text.as_bytes().get(SUFFIX_LEN) != Some(&b'_') {
        return Err(ApiKeyParseError::Shape);
    }
    let tail = text.get(SUFFIX_LEN + 1..).ok_or(ApiKeyParseError::Shape)?;
    Ok((head, tail))
}

impl fmt::Debug for WorkspaceApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "WorkspaceApiKey({}, {}, {}, <redacted>)",
            self.region.code(),
            self.workspace_id,
            self.key_id
        )
    }
}

/// Decodes exactly 43 characters of unpadded base64url into 32 bytes.
///
/// Hand-rolled rather than pulling a dependency into a contract crate. The two
/// residual bits of the 258-bit encoding must be zero, or the value is not a
/// canonical encoding of 32 bytes.
fn decode_base64url_32(text: &str) -> Option<[u8; 32]> {
    const fn symbol(byte: u8) -> Option<u32> {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        Some(value as u32)
    }
    if text.len() != 43 {
        return None;
    }
    let mut out = [0u8; 32];
    let mut accumulator: u32 = 0;
    let mut bits: u32 = 0;
    let mut written = 0usize;
    for byte in text.bytes() {
        accumulator = (accumulator << 6) | symbol(byte)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            let value = u8::try_from((accumulator >> bits) & 0xff).ok()?;
            *out.get_mut(written)? = value;
            written += 1;
        }
    }
    if written != 32 || accumulator & 0x03 != 0 {
        return None;
    }
    Some(out)
}
