//! Content and page digests.
//!
//! Two hash functions, split by contract (D-05):
//!
//! * a **body** digest is SHA-256 of the canonical plaintext, because
//!   `sha256:<hex>` is the public body contract and cannot move. That type is
//!   [`aex_wire::ids::ContentHash`], re-exported here under the domain's name so
//!   exactly one body-digest type exists in the workspace;
//! * a **page** digest is BLAKE3 over hand-written canonical page bytes,
//!   because the conventions pin BLAKE3 for Merkle pages. Page digests are
//!   internal and have no public rendering contract, so they live here.
//!
//! Neither digest ever sees a `serde` encoding. A `serde` bump must not be able
//! to move a persisted hash, so every canonical byte builder in this crate is
//! written out by hand.

use core::fmt;

/// SHA-256 of a canonical plaintext body, rendered `sha256:<64 lowercase hex>`.
///
/// This is `aex_wire`'s public content hash, not a second type: a body digest
/// crosses the customer boundary, so the wire owns its grammar.
pub type ContentDigest = aex_wire::ids::ContentHash;

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

    /// Parses the `blake3:<64 lowercase hex>` form.
    ///
    /// # Errors
    ///
    /// Returns [`PageDigestError`] when the prefix, length or alphabet is wrong.
    pub fn parse(text: &str) -> Result<Self, PageDigestError> {
        let hex = text
            .strip_prefix(Self::PREFIX)
            .ok_or(PageDigestError::Prefix)?;
        let bytes = hex.as_bytes();
        if bytes.len() != 64 {
            return Err(PageDigestError::Length { found: bytes.len() });
        }
        let mut out = [0_u8; 32];
        for (index, chunk) in bytes.chunks_exact(2).enumerate() {
            let high = hex_value(chunk[0]).ok_or(PageDigestError::Character {
                position: index * 2,
            })?;
            let low = hex_value(chunk[1]).ok_or(PageDigestError::Character {
                position: index * 2 + 1,
            })?;
            out[index] = (high << 4) | low;
        }
        Ok(Self(out))
    }
}

impl fmt::Display for PageDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(Self::PREFIX)?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for PageDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

/// The CRC32C an object store records alongside a stored object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Crc32c(pub u32);

/// Why a page digest failed to parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PageDigestError {
    /// The algorithm prefix was absent or wrong.
    #[error("expected page digest prefix `blake3:`")]
    Prefix,
    /// The hex body was not exactly 64 characters.
    #[error("page digest body must be 64 hex characters, found {found}")]
    Length {
        /// Observed body length.
        found: usize,
    },
    /// A body character was not lowercase hex.
    #[error("page digest body character {position} is not lowercase hex")]
    Character {
        /// Zero-based index of the offending character.
        position: usize,
    },
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentDigest, PageDigest, PageDigestError};

    #[test]
    fn content_digest_matches_the_known_sha256_vector() {
        assert_eq!(
            ContentDigest::of(b"abc").to_wire(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn page_digest_uses_blake3_and_is_distinct_from_the_body_digest() {
        let page = PageDigest::of(b"abc");
        assert_ne!(page.as_bytes(), ContentDigest::of(b"abc").as_bytes());
        assert_eq!(PageDigest::parse(&page.to_string()), Ok(page));
    }

    #[test]
    fn page_digest_parsing_is_strict() {
        assert_eq!(PageDigest::parse("sha256:00"), Err(PageDigestError::Prefix));
        assert_eq!(
            PageDigest::parse("blake3:00"),
            Err(PageDigestError::Length { found: 2 })
        );
        let upper = format!("blake3:{}", "A".repeat(64));
        assert_eq!(
            PageDigest::parse(&upper),
            Err(PageDigestError::Character { position: 0 })
        );
    }
}
