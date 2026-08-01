//! Where a body physically lives, and the boundary that decides it.
//!
//! Placement is an internal storage decision, never a customer-visible one.
//! Crossing the boundary never fails a request and never changes a root: the
//! object locator is not an input to any digest, so a body may migrate between
//! inline and object storage without moving a single hash.

use core::fmt;

use crate::digest::{ContentDigest, Crc32c};

/// Largest canonical body that may be stored inline.
pub const INLINE_PLACEMENT_MAX_BYTES: u64 = 32_768;

/// Largest single item any regional application table may hold.
pub const APPLICATION_ITEM_MAX_BYTES: u64 = 262_144;

/// Target canonical size of one Merkle tree page.
pub const TREE_PAGE_TARGET_BYTES: u64 = 196_608;

/// Which storage class a canonical length selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlacementClass {
    /// Stored beside its metadata.
    Inline,
    /// Stored as an object with a content-bound unversioned key.
    Object,
}

/// The placement boundary. Monotone in the canonical length, with
/// [`INLINE_PLACEMENT_MAX_BYTES`] inclusive on the inline side.
#[must_use]
pub const fn placement_for(canonical_len: u64) -> PlacementClass {
    if canonical_len <= INLINE_PLACEMENT_MAX_BYTES {
        PlacementClass::Inline
    } else {
        PlacementClass::Object
    }
}

/// An unversioned object key. Content-bound and opaque: the domain never
/// derives one and never parses meaning out of one.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentObjectKey(String);

impl ContentObjectKey {
    /// Longest accepted key.
    pub const MAX_BYTES: usize = 1024;

    /// Wraps an adapter-minted key.
    ///
    /// # Errors
    ///
    /// Returns [`ObjectKeyError`] when the key is empty, over-long, or
    /// contains a byte outside printable ASCII.
    pub fn parse(text: &str) -> Result<Self, ObjectKeyError> {
        if text.is_empty() {
            return Err(ObjectKeyError::Empty);
        }
        if text.len() > Self::MAX_BYTES {
            return Err(ObjectKeyError::TooLong { bytes: text.len() });
        }
        if let Some(position) = text.bytes().position(|byte| !(0x21..=0x7e).contains(&byte)) {
            return Err(ObjectKeyError::NotPrintableAscii { position });
        }
        Ok(Self(text.to_owned()))
    }

    /// Borrows the key text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for ContentObjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentObjectKey({})", self.0)
    }
}

/// Why an object key was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ObjectKeyError {
    /// The key was the empty string.
    #[error("object key must not be empty")]
    Empty,
    /// The key exceeded [`ContentObjectKey::MAX_BYTES`].
    #[error("object key must be at most {max} bytes, found {bytes}", max = ContentObjectKey::MAX_BYTES)]
    TooLong {
        /// Observed length in bytes.
        bytes: usize,
    },
    /// A byte outside printable ASCII appeared.
    #[error("object key byte {position} is not printable ASCII")]
    NotPrintableAscii {
        /// Zero-based index of the offending byte.
        position: usize,
    },
}

/// Where a body actually is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Placement {
    /// Stored beside its metadata.
    Inline,
    /// Stored as an object under a content-bound unversioned key.
    Object {
        /// The unversioned object key.
        key: ContentObjectKey,
        /// The checksum the object store recorded.
        checksum: Crc32c,
    },
}

impl Placement {
    /// Which class this placement belongs to.
    #[must_use]
    pub const fn class(&self) -> PlacementClass {
        match self {
            Self::Inline => PlacementClass::Inline,
            Self::Object { .. } => PlacementClass::Object,
        }
    }
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
    pub workspace: crate::wire_pending::WorkspaceId,
    /// SHA-256 of the canonical plaintext, measured before compression and
    /// encryption.
    pub digest: ContentDigest,
    /// Plaintext length in bytes.
    pub size_bytes: u64,
    /// Declared media type, when one is known.
    pub media_type: Option<crate::wire_pending::MediaType>,
    /// Where the body physically lives.
    pub placement: Placement,
    /// The ciphertext identity.
    pub ciphertext: CiphertextIdentity,
    /// When the descriptor was first written.
    pub created_at: crate::wire_pending::Timestamp,
}

impl ContentDescriptor {
    /// Whether the placement class agrees with the plaintext length.
    ///
    /// The two may legitimately disagree only while a body is migrating, which
    /// the domain never initiates; a descriptor that fails this predicate has
    /// been assembled incorrectly by an adapter.
    #[must_use]
    pub fn placement_matches_size(&self) -> bool {
        self.placement.class() == placement_for(self.size_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        placement_for, ContentObjectKey, ObjectKeyError, PlacementClass,
        INLINE_PLACEMENT_MAX_BYTES,
    };

    #[test]
    fn the_inline_boundary_is_inclusive_at_32768() {
        assert_eq!(placement_for(0), PlacementClass::Inline);
        assert_eq!(
            placement_for(INLINE_PLACEMENT_MAX_BYTES),
            PlacementClass::Inline
        );
        assert_eq!(
            placement_for(INLINE_PLACEMENT_MAX_BYTES + 1),
            PlacementClass::Object
        );
        assert_eq!(placement_for(u64::MAX), PlacementClass::Object);
    }

    #[test]
    fn object_keys_are_validated() {
        assert!(ContentObjectKey::parse("wks/abc/0123").is_ok());
        assert_eq!(ContentObjectKey::parse(""), Err(ObjectKeyError::Empty));
        assert_eq!(
            ContentObjectKey::parse("a b"),
            Err(ObjectKeyError::NotPrintableAscii { position: 1 })
        );
    }
}
