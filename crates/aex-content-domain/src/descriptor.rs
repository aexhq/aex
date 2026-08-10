//! What a stored body claims about itself.

use core::fmt;

use aex_wire::ids::WorkspaceId;
use aex_wire::types::Timestamp;

use crate::digest::ContentDigest;
use crate::placement::{Placement, PlacementClass, placement_for};

/// A validated `type/subtype` media type.
///
/// Parameters are rejected rather than dropped: a descriptor stores exactly what
/// it was told, and two spellings of the same type must not both be storable.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MediaType(String);

impl MediaType {
    /// Longest accepted media type.
    pub const MAX_BYTES: usize = 255;

    /// Parses a `type/subtype` media type.
    ///
    /// # Errors
    ///
    /// Returns [`MediaTypeError`] when the value is empty, over-long, carries a
    /// parameter, is not exactly one `/`-separated pair, or contains a byte
    /// outside the RFC 6838 restricted-name set.
    pub fn parse(text: &str) -> Result<Self, MediaTypeError> {
        if text.is_empty() {
            return Err(MediaTypeError::Empty);
        }
        if text.len() > Self::MAX_BYTES {
            return Err(MediaTypeError::TooLong { bytes: text.len() });
        }
        if text.contains(';') {
            return Err(MediaTypeError::Parameterized);
        }
        let mut parts = text.split('/');
        let (Some(kind), Some(subtype), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(MediaTypeError::NotTypeSubtype);
        };
        for part in [kind, subtype] {
            if part.is_empty() {
                return Err(MediaTypeError::NotTypeSubtype);
            }
            if let Some(position) = part.bytes().position(|byte| !is_restricted_name_byte(byte)) {
                return Err(MediaTypeError::Character { position });
            }
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrows the media type text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

const fn is_restricted_name_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'-' | b'^' | b'_' | b'.' | b'+'
        )
}

impl fmt::Display for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Debug for MediaType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MediaType({})", self.0)
    }
}

/// Why a media type was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MediaTypeError {
    /// The value was the empty string.
    #[error("media type must not be empty")]
    Empty,
    /// The value exceeded [`MediaType::MAX_BYTES`].
    #[error("media type must be at most {max} bytes, found {bytes}", max = MediaType::MAX_BYTES)]
    TooLong {
        /// Observed byte length.
        bytes: usize,
    },
    /// A `;` parameter section appeared.
    #[error("media type must not carry a parameter")]
    Parameterized,
    /// The value was not exactly one `type/subtype` pair.
    #[error("media type must be exactly one `type/subtype` pair")]
    NotTypeSubtype,
    /// A byte outside the RFC 6838 restricted-name set appeared.
    #[error("media type byte {position} is outside the restricted name set")]
    Character {
        /// Zero-based index of the offending byte within its part.
        position: usize,
    },
}

/// The identity of the ciphertext holding a body.
///
/// Opaque to the domain: it names a wrapped-key generation and a nonce so an
/// adapter can decrypt, and it never participates in a digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CiphertextIdentity {
    /// The key generation the body was wrapped under.
    pub key_generation: u64,
    /// The wrapped data key, as stored.
    pub wrapped_key: Vec<u8>,
    /// The nonce used for the body.
    pub nonce: Vec<u8>,
}

/// What a stored body claims about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDescriptor {
    /// Owning workspace.
    pub workspace: WorkspaceId,
    /// SHA-256 of the canonical plaintext, measured before compression and
    /// encryption.
    pub digest: ContentDigest,
    /// Plaintext length in bytes.
    pub size_bytes: u64,
    /// Declared media type, when one is known.
    pub media_type: Option<MediaType>,
    /// Where the body physically lives.
    pub placement: Placement,
    /// The ciphertext identity.
    pub ciphertext: CiphertextIdentity,
    /// When the descriptor was first written.
    pub created_at: Timestamp,
}

impl ContentDescriptor {
    /// Whether the placement class agrees with the plaintext length.
    ///
    /// **Inline implies small**, not "small implies inline". Equality was the
    /// rule until the registry needed the other direction: a registered payload
    /// is always object-placed however short it is (D-11), because the API never
    /// reads a registry payload and an inline copy would leave
    /// `registry_files_download_create` with no object to sign. The invariant
    /// that matters — an inline body is never larger than the inline ceiling —
    /// is unchanged.
    ///
    /// A descriptor that fails this predicate has been assembled incorrectly by
    /// an adapter; the domain never mints a disagreeing pair.
    #[must_use]
    pub fn placement_matches_size(&self) -> bool {
        self.placement.class() != PlacementClass::Inline
            || placement_for(self.size_bytes) == PlacementClass::Inline
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaType, MediaTypeError};

    #[test]
    fn media_types_are_a_single_restricted_pair() {
        assert_eq!(
            MediaType::parse("application/json").map(|value| value.as_str().to_owned()),
            Ok("application/json".to_owned())
        );
        assert_eq!(MediaType::parse(""), Err(MediaTypeError::Empty));
        assert_eq!(
            MediaType::parse("text/plain; charset=utf-8"),
            Err(MediaTypeError::Parameterized)
        );
        assert_eq!(
            MediaType::parse("application"),
            Err(MediaTypeError::NotTypeSubtype)
        );
        assert_eq!(
            MediaType::parse("a/b/c"),
            Err(MediaTypeError::NotTypeSubtype)
        );
        assert_eq!(
            MediaType::parse("Application/json"),
            Err(MediaTypeError::Character { position: 0 })
        );
    }
}
