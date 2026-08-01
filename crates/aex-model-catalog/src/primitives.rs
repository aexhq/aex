//! Bounded text, opaque byte payloads and the blake3 digest newtype.
//!
//! Everything here is owned by this crate. The wire vocabulary this crate
//! *consumes* — `ProviderId`, `ModelSelection`, `ContentHash`, `Timestamp`,
//! `CanonicalJson`, `ResourceName`, `ToolCallId`, `WorkspaceId`,
//! `ProviderCredentialId`, `ErrorCode`, `to_jcs_bytes` — comes from `aex-wire`
//! and is never redefined.

use core::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// bounded text
// ---------------------------------------------------------------------------

/// A `String` that cannot exceed `N` UTF-8 bytes.
///
/// Every text field that crosses a provider boundary in this stream is bounded,
/// so a hostile or merely broken provider cannot make a buffer grow without
/// limit.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoundedString<const N: usize>(String);

/// The single failure a [`BoundedString`] construction can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("string of {seen} bytes exceeds the {limit}-byte bound")]
pub struct BoundError {
    /// The declared maximum, in UTF-8 bytes.
    pub limit: usize,
    /// The length of the rejected value, in UTF-8 bytes.
    pub seen: usize,
}

impl<const N: usize> BoundedString<N> {
    /// Builds a bounded string, rejecting anything longer than `N` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`BoundError`] when the value exceeds `N` UTF-8 bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, BoundError> {
        let value = value.into();
        if value.len() > N {
            return Err(BoundError {
                limit: N,
                seen: value.len(),
            });
        }
        Ok(Self(value))
    }

    /// Builds a bounded string by truncating on a UTF-8 character boundary.
    ///
    /// Used only by the redactor, where the alternative to a bounded prefix is
    /// dropping a diagnostic entirely.
    #[must_use]
    pub fn truncating(value: &str) -> Self {
        if value.len() <= N {
            return Self(value.to_owned());
        }
        let mut end = N;
        while end > 0 && !value.is_char_boundary(end) {
            end -= 1;
        }
        Self(value[..end].to_owned())
    }

    /// The borrowed contents.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length in UTF-8 bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the value is the empty string.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The declared bound.
    #[must_use]
    pub const fn bound() -> usize {
        N
    }
}

impl<const N: usize> fmt::Debug for BoundedString<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, formatter)
    }
}

impl<const N: usize> fmt::Display for BoundedString<N> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<const N: usize> Serialize for BoundedString<N> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de, const N: usize> Deserialize<'de> for BoundedString<N> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::new(raw).map_err(D::Error::custom)
    }
}

/// The exact provider-native model id, carried verbatim. There is no inference
/// from a model name to a provider anywhere in this crate.
pub type ModelSlug = BoundedString<256>;

/// A provider-assigned request identifier, where the provider publishes one.
pub type ProviderRequestId = BoundedString<80>;

/// A provider-assigned tool-call id, carried verbatim.
///
/// Deliberately **not** `aex_wire::ToolCallId`. That is an AEX-minted prefixed
/// UUID; a provider id such as `Moonshot`'s `search:0`, `OpenAI`'s `call_abc123` or
/// Anthropic's `toolu_01…` cannot be expressed in it. Recorded as a cross-stream
/// note in `references/rewrite/providers.md`.
pub type ToolCallId = BoundedString<128>;

/// A tool name as declared to the provider. The workspace registered-name
/// grammar is a superset of every provider's own tool-name grammar; the
/// per-provider narrowing lives in `document::NamePattern`.
pub type ToolName = aex_wire::ResourceName;

// ---------------------------------------------------------------------------
// blake3 digests
// ---------------------------------------------------------------------------

/// A blake3-256 digest, rendered as 64 lowercase hexadecimal characters.
///
/// `aex_wire::ContentHash` is SHA-256 and renders `sha256:<hex>`; the catalog
/// document digest is pinned to blake3-256 by plan 08 D-01, so it is a distinct
/// type rather than a differently-computed value of the same one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Blake3Digest([u8; 32]);

/// A malformed hexadecimal digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("expected 64 lowercase hexadecimal characters")]
pub struct HexError;

impl Blake3Digest {
    /// Hashes `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

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

    /// The 64-character lowercase hexadecimal rendering.
    #[must_use]
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
        out
    }

    /// Parses the 64-character lowercase hexadecimal rendering.
    ///
    /// # Errors
    ///
    /// Returns [`HexError`] for the wrong length or a non-lowercase-hex digit.
    pub fn from_hex(text: &str) -> Result<Self, HexError> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return Err(HexError);
        }
        let mut out = [0u8; 32];
        for (index, pair) in bytes.chunks_exact(2).enumerate() {
            let high = hex_value(pair[0])?;
            let low = hex_value(pair[1])?;
            out[index] = (high << 4) | low;
        }
        Ok(Self(out))
    }
}

const fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + nibble - 10) as char,
    }
}

const fn hex_value(byte: u8) -> Result<u8, HexError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(HexError),
    }
}

impl fmt::Debug for Blake3Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Blake3Digest({})", self.to_hex())
    }
}

impl fmt::Display for Blake3Digest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for Blake3Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Blake3Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::from_hex(&raw).map_err(D::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// opaque byte payloads
// ---------------------------------------------------------------------------

/// Base64 (standard alphabet, padded) serde for opaque provider round-trip
/// material. A byte array rendered as a JSON number sequence would triple the
/// journal size and lose its opacity.
pub mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes bytes as a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Propagates the serializer's own failure only.
    pub fn serialize<S: Serializer>(
        value: &bytes::Bytes,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(value.as_ref()))
    }

    /// Deserializes a padded standard-alphabet base64 string.
    ///
    /// # Errors
    ///
    /// Returns a deserializer error for a malformed encoding.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<bytes::Bytes, D::Error> {
        use serde::de::Error as _;
        let raw = String::deserialize(deserializer)?;
        let decoded = STANDARD.decode(raw.as_bytes()).map_err(D::Error::custom)?;
        Ok(bytes::Bytes::from(decoded))
    }
}

#[cfg(test)]
mod tests {
    use super::{Blake3Digest, BoundedString, HexError};

    #[test]
    fn bounded_string_rejects_an_over_long_value() {
        let error = BoundedString::<4>::new("abcde").expect_err("five bytes exceeds four");
        assert_eq!(error.limit, 4);
        assert_eq!(error.seen, 5);
    }

    #[test]
    fn bounded_string_accepts_exactly_the_bound() {
        let value = BoundedString::<4>::new("abcd").expect("four bytes fits four");
        assert_eq!(value.as_str(), "abcd");
        assert_eq!(BoundedString::<4>::bound(), 4);
    }

    #[test]
    fn truncating_never_splits_a_utf8_scalar() {
        // `é` is two bytes, so a three-byte bound must drop it whole.
        let value = BoundedString::<3>::truncating("aéb");
        assert_eq!(value.as_str(), "aé");
        assert_eq!(value.len(), 3);
    }

    #[test]
    fn truncating_backs_off_to_a_boundary() {
        let value = BoundedString::<2>::truncating("aéb");
        assert_eq!(value.as_str(), "a");
    }

    #[test]
    fn bounded_string_deserialization_enforces_the_bound() {
        let ok: BoundedString<4> = serde_json::from_str("\"abcd\"").expect("fits");
        assert_eq!(ok.as_str(), "abcd");
        serde_json::from_str::<BoundedString<4>>("\"abcde\"").expect_err("exceeds the bound");
    }

    #[test]
    fn digest_hex_round_trips() {
        let digest = Blake3Digest::of(b"aex");
        let hex = digest.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(Blake3Digest::from_hex(&hex), Ok(digest));
    }

    #[test]
    fn digest_rejects_uppercase_and_short_hex() {
        assert_eq!(Blake3Digest::from_hex(&"A".repeat(64)), Err(HexError));
        assert_eq!(Blake3Digest::from_hex("abcd"), Err(HexError));
    }

    #[test]
    fn digest_is_not_the_sha256_content_hash() {
        let blake = Blake3Digest::of(b"aex").to_hex();
        let sha = aex_wire::ContentHash::of(b"aex").to_wire();
        assert_ne!(format!("sha256:{blake}"), sha);
    }
}
