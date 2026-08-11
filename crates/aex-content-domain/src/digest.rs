//! Content-body digests and object checksums.

/// SHA-256 of canonical plaintext, rendered `sha256:<64 lowercase hex>`.
///
/// This is `aex_wire`'s public content hash, not a second type.
pub type ContentDigest = aex_wire::ids::ContentHash;

/// The CRC32C an object store records alongside a stored object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Crc32c(pub u32);

#[cfg(test)]
mod tests {
    use super::ContentDigest;

    #[test]
    fn content_digest_matches_the_known_sha256_vector() {
        assert_eq!(
            ContentDigest::of(b"abc").to_wire(),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
