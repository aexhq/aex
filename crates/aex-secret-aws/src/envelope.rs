//! The envelope: message framing, key derivation and the AEAD itself.
//!
//! This is the arm OD-33 selected. `aws-esdk` 1.2.4 pins `aws-lc-sys ^0.39`,
//! which cannot unify with the workspace's `aws-lc-rs 1.17.3` →
//! `aws-lc-sys 0.43.0`; adopting it would put two copies of a security-critical
//! native crypto library in one link graph. The key hierarchy here is the same
//! one the ESDK would have used — KMS root key, per-workspace branch key,
//! per-message derived wrapping key, per-message AEAD — with the same
//! encryption-context-as-AAD binding and the same primitive library. The only
//! thing given up is the ESDK's interoperable message format, and nothing
//! outside this workspace ever reads these bytes.
//!
//! The frame is:
//!
//! ```text
//! "AEX1" ‖ version:u8 ‖ branch_key_id_len:u16 ‖ branch_key_id
//!        ‖ branch_key_version:16 ‖ salt:16 ‖ nonce:12 ‖ ciphertext ‖ tag:16
//! ```

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::hkdf::{HKDF_SHA512, Salt};
use zeroize::Zeroizing;

/// The frame magic.
pub const MAGIC: &[u8; 4] = b"AEX1";

/// The frame version. A future format change is a new number, never a tolerated
/// parse.
pub const VERSION: u8 = 1;

/// The HKDF salt width.
pub const SALT_BYTES: usize = 16;

/// The AEAD nonce width.
pub const NONCE_BYTES: usize = 12;

/// The branch-key version identifier width.
pub const KEY_VERSION_BYTES: usize = 16;

/// The AEAD tag width.
pub const TAG_BYTES: usize = 16;

/// The derived wrapping-key width.
pub const KEY_BYTES: usize = 32;

/// The HKDF `info` suffix, so material derived here can never collide with
/// material derived for another purpose under the same branch key.
pub const HKDF_INFO_SUFFIX: &[u8] = b"aex-hkdf-v1";

/// Why sealing or opening failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// The frame was not this format.
    #[error("the envelope frame is malformed: {reason}")]
    Malformed {
        /// What was wrong.
        reason: &'static str,
    },
    /// The frame declared a version this build does not implement.
    #[error("the envelope declares version {found}; this build implements {VERSION}")]
    Version {
        /// What the frame declared.
        found: u8,
    },
    /// The AEAD refused the ciphertext.
    ///
    /// Reported identically for a tampered ciphertext, a wrong branch key and a
    /// mismatched encryption context, because telling them apart would tell a
    /// prober which of the three it got wrong.
    #[error("the sealed value did not open under this key and context")]
    NotAuthentic,
    /// A primitive refused an input.
    #[error("a cryptographic primitive refused the input")]
    Primitive,
}

/// The branch-key material a message is derived from.
///
/// Held in `Zeroizing`, so a dropped copy does not leave key material in the
/// allocator.
#[derive(Clone)]
pub struct BranchKeyMaterial {
    /// Which branch key.
    pub branch_key_id: String,
    /// Which version of it.
    pub version: [u8; KEY_VERSION_BYTES],
    /// The material itself.
    pub material: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for BranchKeyMaterial {
    /// Prints the identity, never the material.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BranchKeyMaterial")
            .field("branch_key_id", &self.branch_key_id)
            .field("version", &hex::encode(self.version))
            .field("material", &"<redacted>")
            .finish()
    }
}

/// Random bytes, so a test can pin them and production cannot.
pub trait Entropy: Send + Sync {
    /// Fills `bytes`.
    fn fill(&self, bytes: &mut [u8]);
}

/// The system random source.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemEntropy;

impl Entropy for SystemEntropy {
    fn fill(&self, bytes: &mut [u8]) {
        use aws_lc_rs::rand::SecureRandom as _;
        aws_lc_rs::rand::SystemRandom::new()
            .fill(bytes)
            .unwrap_or_else(|_| unreachable!("the system random source does not fail"));
    }
}

/// Seals `plaintext` under `material`, bound to `aad`.
///
/// # Errors
///
/// [`EnvelopeError::Primitive`] when a primitive refuses the input.
pub fn seal(
    material: &BranchKeyMaterial,
    aad: &[u8],
    plaintext: &[u8],
    entropy: &dyn Entropy,
) -> Result<Vec<u8>, EnvelopeError> {
    let mut salt = [0u8; SALT_BYTES];
    entropy.fill(&mut salt);
    let mut nonce = [0u8; NONCE_BYTES];
    entropy.fill(&mut nonce);

    let key = derive(material, &salt, aad)?;
    let mut sealed = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad),
        &mut sealed,
    )
    .map_err(|_| EnvelopeError::Primitive)?;

    let id = material.branch_key_id.as_bytes();
    let id_len = u16::try_from(id.len()).map_err(|_| EnvelopeError::Malformed {
        reason: "a branch key id longer than u16::MAX",
    })?;
    let mut frame = Vec::with_capacity(
        MAGIC.len()
            + 1
            + 2
            + id.len()
            + KEY_VERSION_BYTES
            + SALT_BYTES
            + NONCE_BYTES
            + sealed.len(),
    );
    frame.extend_from_slice(MAGIC);
    frame.push(VERSION);
    frame.extend_from_slice(&id_len.to_le_bytes());
    frame.extend_from_slice(id);
    frame.extend_from_slice(&material.version);
    frame.extend_from_slice(&salt);
    frame.extend_from_slice(&nonce);
    frame.extend_from_slice(&sealed);
    Ok(frame)
}

/// What a frame says about itself, before anything is opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// Which branch key sealed it.
    pub branch_key_id: String,
    /// Which version of that key.
    pub version: [u8; KEY_VERSION_BYTES],
}

/// Reads the header without opening the message.
///
/// The header is what selects the key, so it has to be readable before the AEAD
/// runs. It is authenticated only in the sense that a header naming another key
/// simply fails to open.
///
/// # Errors
///
/// [`EnvelopeError::Malformed`] or [`EnvelopeError::Version`].
pub fn header(frame: &[u8]) -> Result<FrameHeader, EnvelopeError> {
    let (header, _) = split(frame)?;
    Ok(header)
}

/// Opens a frame under `material`, bound to `aad`.
///
/// # Errors
///
/// [`EnvelopeError::NotAuthentic`] when the tag does not verify, which covers a
/// tampered ciphertext, a wrong key and a mismatched context alike.
pub fn open(
    material: &BranchKeyMaterial,
    aad: &[u8],
    frame: &[u8],
) -> Result<Zeroizing<Vec<u8>>, EnvelopeError> {
    let (parsed, body) = split(frame)?;
    if parsed.branch_key_id != material.branch_key_id || parsed.version != material.version {
        return Err(EnvelopeError::NotAuthentic);
    }
    let salt: [u8; SALT_BYTES] = body[..SALT_BYTES]
        .try_into()
        .map_err(|_| EnvelopeError::Malformed { reason: "no salt" })?;
    let nonce: [u8; NONCE_BYTES] = body[SALT_BYTES..SALT_BYTES + NONCE_BYTES]
        .try_into()
        .map_err(|_| EnvelopeError::Malformed { reason: "no nonce" })?;
    let sealed = &body[SALT_BYTES + NONCE_BYTES..];
    if sealed.len() < TAG_BYTES {
        return Err(EnvelopeError::Malformed {
            reason: "the frame is shorter than its own tag",
        });
    }

    let key = derive(material, &salt, aad)?;
    let mut buffer = Zeroizing::new(sealed.to_vec());
    let opened = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad),
            &mut buffer,
        )
        .map_err(|_| EnvelopeError::NotAuthentic)?;
    Ok(Zeroizing::new(opened.to_vec()))
}

fn split(frame: &[u8]) -> Result<(FrameHeader, &[u8]), EnvelopeError> {
    let minimum = MAGIC.len() + 1 + 2;
    if frame.len() < minimum {
        return Err(EnvelopeError::Malformed {
            reason: "shorter than a header",
        });
    }
    if &frame[..MAGIC.len()] != MAGIC {
        return Err(EnvelopeError::Malformed {
            reason: "not an AEX envelope",
        });
    }
    let version = frame[MAGIC.len()];
    if version != VERSION {
        return Err(EnvelopeError::Version { found: version });
    }
    let id_len = usize::from(u16::from_le_bytes([frame[5], frame[6]]));
    let id_end = minimum + id_len;
    let body_start = id_end + KEY_VERSION_BYTES;
    if frame.len() < body_start + SALT_BYTES + NONCE_BYTES + TAG_BYTES {
        return Err(EnvelopeError::Malformed {
            reason: "truncated frame",
        });
    }
    let branch_key_id = String::from_utf8(frame[minimum..id_end].to_vec()).map_err(|_| {
        EnvelopeError::Malformed {
            reason: "a branch key id that is not UTF-8",
        }
    })?;
    let version_bytes: [u8; KEY_VERSION_BYTES] =
        frame[id_end..body_start]
            .try_into()
            .map_err(|_| EnvelopeError::Malformed {
                reason: "no key version",
            })?;
    Ok((
        FrameHeader {
            branch_key_id,
            version: version_bytes,
        },
        &frame[body_start..],
    ))
}

fn derive(
    material: &BranchKeyMaterial,
    salt: &[u8; SALT_BYTES],
    aad: &[u8],
) -> Result<LessSafeKey, EnvelopeError> {
    let mut info = Vec::with_capacity(aad.len() + HKDF_INFO_SUFFIX.len());
    info.extend_from_slice(aad);
    info.extend_from_slice(HKDF_INFO_SUFFIX);

    let prk = Salt::new(HKDF_SHA512, salt).extract(&material.material);
    let info_parts: [&[u8]; 1] = [&info];
    let okm = prk
        .expand(&info_parts, HkdfKeyLength(KEY_BYTES))
        .map_err(|_| EnvelopeError::Primitive)?;
    let mut derived = Zeroizing::new(vec![0u8; KEY_BYTES]);
    okm.fill(&mut derived)
        .map_err(|_| EnvelopeError::Primitive)?;
    let unbound = UnboundKey::new(&AES_256_GCM, &derived).map_err(|_| EnvelopeError::Primitive)?;
    Ok(LessSafeKey::new(unbound))
}

#[derive(Debug, Clone, Copy)]
struct HkdfKeyLength(usize);

impl aws_lc_rs::hkdf::KeyType for HkdfKeyLength {
    fn len(&self) -> usize {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    use super::{
        BranchKeyMaterial, Entropy, EnvelopeError, KEY_VERSION_BYTES, MAGIC, VERSION, header, open,
        seal,
    };

    /// Pinned entropy, so a frame is byte-stable inside a test.
    struct Pinned(u8);

    impl Entropy for Pinned {
        fn fill(&self, bytes: &mut [u8]) {
            bytes.fill(self.0);
        }
    }

    fn material(id: &str, version: u8) -> BranchKeyMaterial {
        BranchKeyMaterial {
            branch_key_id: id.to_owned(),
            version: [version; KEY_VERSION_BYTES],
            material: Zeroizing::new(vec![0x2a; 32]),
        }
    }

    #[test]
    fn a_sealed_value_opens_under_the_same_key_and_context() {
        let sealed =
            seal(&material("wsp_1", 1), b"context", b"hunter2", &Pinned(7)).expect("seals");
        let opened = open(&material("wsp_1", 1), b"context", &sealed).expect("opens");
        assert_eq!(opened.as_slice(), b"hunter2");
    }

    #[test]
    fn the_frame_declares_its_magic_its_version_and_its_key_identity() {
        let sealed = seal(&material("wsp_1", 3), b"context", b"value", &Pinned(1)).expect("seals");
        assert_eq!(&sealed[..4], MAGIC);
        assert_eq!(sealed[4], VERSION);
        let parsed = header(&sealed).expect("a header");
        assert_eq!(parsed.branch_key_id, "wsp_1");
        assert_eq!(parsed.version, [3; KEY_VERSION_BYTES]);
    }

    #[test]
    fn a_ciphertext_never_contains_its_own_plaintext() {
        let sealed = seal(
            &material("wsp_1", 1),
            b"context",
            b"sk-live-a-real-looking-secret",
            &Pinned(9),
        )
        .expect("seals");
        assert!(
            !sealed.windows(7).any(|window| window == b"sk-live"),
            "the frame leaked its plaintext"
        );
    }

    #[test]
    fn a_value_sealed_under_one_context_never_opens_under_another() {
        let sealed =
            seal(&material("wsp_1", 1), b"context-a", b"value", &Pinned(4)).expect("seals");
        assert_eq!(
            open(&material("wsp_1", 1), b"context-b", &sealed).unwrap_err(),
            EnvelopeError::NotAuthentic
        );
    }

    #[test]
    fn a_value_sealed_under_one_branch_key_never_opens_under_another() {
        let sealed = seal(&material("wsp_1", 1), b"context", b"value", &Pinned(4)).expect("seals");
        assert_eq!(
            open(&material("wsp_2", 1), b"context", &sealed).unwrap_err(),
            EnvelopeError::NotAuthentic,
            "a cross-tenant open must fail rather than return something plausible"
        );
        assert_eq!(
            open(&material("wsp_1", 2), b"context", &sealed).unwrap_err(),
            EnvelopeError::NotAuthentic
        );
    }

    #[test]
    fn a_flipped_bit_anywhere_in_the_frame_is_refused() {
        let sealed = seal(&material("wsp_1", 1), b"context", b"value", &Pinned(4)).expect("seals");
        for index in (0..sealed.len()).step_by(7) {
            let mut tampered = sealed.clone();
            tampered[index] ^= 0x01;
            assert!(
                open(&material("wsp_1", 1), b"context", &tampered).is_err(),
                "byte {index} was not authenticated"
            );
        }
    }

    #[test]
    fn a_frame_from_another_format_or_version_is_refused_rather_than_parsed() {
        assert!(matches!(
            header(b"NOPE").unwrap_err(),
            EnvelopeError::Malformed { .. }
        ));
        let mut sealed = seal(&material("wsp_1", 1), b"c", b"v", &Pinned(1)).expect("seals");
        sealed[4] = 9;
        assert_eq!(
            header(&sealed).unwrap_err(),
            EnvelopeError::Version { found: 9 }
        );
    }

    #[test]
    fn a_truncated_frame_is_refused_rather_than_indexed_into() {
        let sealed = seal(&material("wsp_1", 1), b"c", b"v", &Pinned(1)).expect("seals");
        for length in 0..sealed.len() {
            let _ = header(&sealed[..length]);
            let _ = open(&material("wsp_1", 1), b"c", &sealed[..length]);
        }
    }

    #[test]
    fn key_material_never_prints_itself() {
        let printed = format!("{:?}", material("wsp_1", 1));
        assert!(printed.contains("<redacted>"), "{printed}");
        assert!(!printed.contains("42"), "{printed}");
    }
}
