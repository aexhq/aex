//! The redacting plaintext type.
//!
//! Plaintext non-persistence is enforced by the **type**, not by discipline.
//! [`SecretPlaintext`] deliberately does not implement `Serialize`, `Clone`,
//! `PartialEq` or `Deref`; its `Debug` and `Display` render a fixed redaction
//! marker; and its buffer is zeroized when it drops. The only way to reach the
//! bytes is [`SecretPlaintext::expose_for_encryption`], whose name is the point:
//! a reviewer sees every site that touches plaintext by grepping one symbol.

use core::fmt;

use aex_wire::ids::{ContentHash, ProviderCredentialId, WorkspaceId};
use zeroize::Zeroizing;

/// Domain separation for the provider-credential fingerprint.
const FINGERPRINT_DOMAIN: &[u8] = b"aex.provider-credential.fingerprint.v1";

/// The redaction marker every rendering of a plaintext produces.
pub const REDACTED: &str = "<redacted>";

/// A secret value in the clear, on its way to an encryptor and nowhere else.
pub struct SecretPlaintext(Zeroizing<Vec<u8>>);

impl SecretPlaintext {
    /// Largest accepted plaintext.
    ///
    /// The durable limit is `secret.value_bytes`; this is the structural ceiling
    /// that keeps an unbounded body from reaching an encryptor at all.
    pub const MAX_BYTES: usize = 65_536;

    /// Takes ownership of a plaintext.
    ///
    /// # Errors
    ///
    /// Returns [`PlaintextError`] for an empty or over-long value.
    pub fn new(bytes: Vec<u8>) -> Result<Self, PlaintextError> {
        if bytes.is_empty() {
            return Err(PlaintextError::Empty);
        }
        if bytes.len() > Self::MAX_BYTES {
            return Err(PlaintextError::TooLong { bytes: bytes.len() });
        }
        Ok(Self(Zeroizing::new(bytes)))
    }

    /// The bytes, for one purpose only.
    #[must_use]
    pub fn expose_for_encryption(&self) -> &[u8] {
        &self.0
    }

    /// How many bytes the plaintext holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Always `false` for a constructed value; present so `len` does not stand
    /// alone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The published one-way fingerprint of a BYOK provider credential.
    ///
    /// This is the value the wire's `ProviderCredential.fingerprint` carries, and
    /// it is derived **once, at registration, by the only component that ever
    /// holds the plaintext**. It is persisted, never recomputed at read time: a
    /// read path that could recompute it would need the plaintext, which is the
    /// whole property the custody boundary exists to prevent.
    ///
    /// The construction is a domain-separated SHA-256 over length-prefixed
    /// identifiers followed by the value:
    ///
    /// ```text
    /// sha256( "aex.provider-credential.fingerprint.v1"
    ///         ‖ len(workspace) ‖ workspace
    ///         ‖ len(credential) ‖ credential
    ///         ‖ plaintext )
    /// ```
    ///
    /// Two properties follow, and both are asserted:
    ///
    /// * it is **not derivable back to the secret** — SHA-256 is one-way, and no
    ///   byte, length hint or substring of the value survives into the digest;
    /// * it is **salted by the binding it describes** — the same key registered
    ///   in another workspace, or under another credential id, produces a
    ///   different fingerprint, so one precomputed table cannot cover the fleet
    ///   and two customers cannot learn they hold the same key.
    ///
    /// The length prefixes stop `(workspace ‖ credential)` being ambiguous, which
    /// would let a crafted pair collide with another binding's salt.
    #[must_use]
    pub fn credential_fingerprint(
        &self,
        workspace: WorkspaceId,
        credential: ProviderCredentialId,
    ) -> ContentHash {
        use sha2::Digest as _;

        let mut hasher = sha2::Sha256::new();
        hasher.update(FINGERPRINT_DOMAIN);
        for identifier in [workspace.to_string(), credential.to_string()] {
            let bytes = identifier.as_bytes();
            let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
            hasher.update(length.to_be_bytes());
            hasher.update(bytes);
        }
        hasher.update(&self.0);
        ContentHash::from_bytes(hasher.finalize().into())
    }
}

impl fmt::Debug for SecretPlaintext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SecretPlaintext({REDACTED})")
    }
}

impl fmt::Display for SecretPlaintext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REDACTED)
    }
}

/// Why a plaintext was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PlaintextError {
    /// The value was empty.
    #[error("secret value must not be empty")]
    Empty,
    /// The value exceeded [`SecretPlaintext::MAX_BYTES`].
    #[error("secret value must be at most {max} bytes, found {bytes}", max = SecretPlaintext::MAX_BYTES)]
    TooLong {
        /// Observed byte length.
        bytes: usize,
    },
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroize as _;

    use super::{PlaintextError, REDACTED, SecretPlaintext};

    #[test]
    fn construction_is_bounded() {
        assert_eq!(
            SecretPlaintext::new(Vec::new()).err(),
            Some(PlaintextError::Empty)
        );
        assert_eq!(
            SecretPlaintext::new(vec![1; SecretPlaintext::MAX_BYTES + 1]).err(),
            Some(PlaintextError::TooLong {
                bytes: SecretPlaintext::MAX_BYTES + 1
            })
        );
        let value = SecretPlaintext::new(b"hunter2".to_vec()).expect("accepted");
        assert_eq!(value.len(), 7);
        assert!(!value.is_empty());
        assert_eq!(value.expose_for_encryption(), b"hunter2");
    }

    #[test]
    fn neither_rendering_contains_a_plaintext_byte() {
        let value = SecretPlaintext::new(b"hunter2".to_vec()).expect("accepted");
        assert_eq!(format!("{value}"), REDACTED);
        assert_eq!(format!("{value:?}"), format!("SecretPlaintext({REDACTED})"));
        assert!(!format!("{value}{value:?}").contains("hunter2"));
    }

    #[test]
    fn the_chosen_buffer_zeroizes() {
        // Observing memory after a drop needs `unsafe`, which the workspace
        // forbids, so the drop guarantee is proved on the wrapper the field
        // actually uses: `Zeroizing<Vec<u8>>` clears its buffer on `zeroize`,
        // and `Zeroizing` calls exactly that from its `Drop`. The companion
        // source scan in `tests/security.rs` proves the field still uses it.
        let mut buffer = zeroize::Zeroizing::new(b"hunter2".to_vec());
        buffer.zeroize();
        assert!(buffer.iter().all(|byte| *byte == 0));
    }
}
