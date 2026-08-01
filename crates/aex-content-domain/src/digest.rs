//! Content and page digests.
//!
//! Two hash functions, split by contract (D-05):
//!
//! * a **body** digest is SHA-256 of the canonical plaintext, because
//!   `sha256:<hex>` is the public body contract and cannot move;
//! * a **page** digest is BLAKE3 over hand-written canonical page bytes,
//!   because the conventions pin BLAKE3 for Merkle pages.
//!
//! Neither digest ever sees a serde encoding. A serde bump must not be able to
//! move a persisted hash, so every canonical byte builder in this crate is
//! written out by hand.

use core::fmt;

use sha2::Digest as _;

/// SHA-256 of a canonical plaintext body.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentDigest([u8; 32]);

impl ContentDigest {
    /// The public rendering prefix.
    pub const PREFIX: &'static str = "sha256:";

    /// Wraps precomputed digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Hashes a canonical plaintext body.
    #[must_use]
    pub fn of(body: &[u8]) -> Self {
        let mut hasher = sha2::Sha256::new();
        hasher.update(body);
        let out = hasher.finalize();
        let mut bytes = [0_u8; 32];
        bytes.copy_from_slice(&out);
        Self(bytes)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses the public `sha256:<64 lower-case hex>` form.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError`] when the prefix, length or alphabet is wrong.
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let hex = text
            .strip_prefix(Self::PREFIX)
            .ok_or(DigestError::Prefix { expected: Self::PREFIX })?;
        parse_hex32(hex).map(Self)
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::PREFIX)?;
        write_hex(&self.0, f)
    }
}

impl fmt::Debug for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// BLAKE3 of canonical Merkle page bytes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageDigest([u8; 32]);

impl PageDigest {
    /// The internal rendering prefix.
    pub const PREFIX: &'static str = "blake3:";

    /// Wraps precomputed digest bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Hashes canonical page bytes.
    #[must_use]
    pub fn of(canonical_page: &[u8]) -> Self {
        Self(*blake3::hash(canonical_page).as_bytes())
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Parses the `blake3:<64 lower-case hex>` form.
    ///
    /// # Errors
    ///
    /// Returns [`DigestError`] when the prefix, length or alphabet is wrong.
    pub fn parse(text: &str) -> Result<Self, DigestError> {
        let hex = text
            .strip_prefix(Self::PREFIX)
            .ok_or(DigestError::Prefix { expected: Self::PREFIX })?;
        parse_hex32(hex).map(Self)
    }
}

impl fmt::Display for PageDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(Self::PREFIX)?;
        write_hex(&self.0, f)
    }
}

impl fmt::Debug for PageDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The CRC32C an object store records alongside a stored object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Crc32c(pub u32);

/// Why a digest failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DigestError {
    /// The algorithm prefix was absent or wrong.
    #[error("expected digest prefix `{expected}`")]
    Prefix {
        /// The prefix the target type requires.
        expected: &'static str,
    },
    /// The hex body was not exactly 64 characters.
    #[error("digest body must be 64 hex characters, found {found}")]
    Length {
        /// Observed body length.
        found: usize,
    },
    /// A body character was not lower-case hex.
    #[error("digest body character {position} is not lower-case hex")]
    Character {
        /// Zero-based index of the offending character.
        position: usize,
    },
}

fn parse_hex32(hex: &str) -> Result<[u8; 32], DigestError> {
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return Err(DigestError::Length { found: bytes.len() });
    }
    let mut out = [0_u8; 32];
    for (index, chunk) in bytes.chunks_exact(2).enumerate() {
        let high = hex_value(chunk[0]).ok_or(DigestError::Character { position: index * 2 })?;
        let low =
            hex_value(chunk[1]).ok_or(DigestError::Character { position: index * 2 + 1 })?;
        out[index] = (high << 4) | low;
    }
    Ok(out)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn write_hex(bytes: &[u8; 32], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02x}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ContentDigest, DigestError, PageDigest};

    #[test]
    fn content_digest_renders_and_parses_the_public_form() {
        let digest = ContentDigest::of(b"hello");
        let text = digest.to_string();
        assert!(text.starts_with("sha256:"));
        assert_eq!(text.len(), 7 + 64);
        assert_eq!(ContentDigest::parse(&text), Ok(digest));
    }

    #[test]
    fn content_digest_matches_the_known_sha256_vector() {
        assert_eq!(
            ContentDigest::of(b"abc").to_string(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn digest_parsing_is_strict() {
        assert_eq!(
            ContentDigest::parse("blake3:00"),
            Err(DigestError::Prefix { expected: "sha256:" })
        );
        assert_eq!(
            ContentDigest::parse("sha256:00"),
            Err(DigestError::Length { found: 2 })
        );
        let upper = format!("sha256:{}", "A".repeat(64));
        assert_eq!(
            ContentDigest::parse(&upper),
            Err(DigestError::Character { position: 0 })
        );
    }

    #[test]
    fn page_digest_uses_blake3_and_is_distinct_from_the_body_digest() {
        let page = PageDigest::of(b"abc");
        assert_ne!(page.as_bytes(), ContentDigest::of(b"abc").as_bytes());
        assert_eq!(PageDigest::parse(&page.to_string()), Ok(page));
    }
}
