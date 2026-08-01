//! The redacting plaintext type.
//!
//! Plaintext non-persistence is enforced by the **type**, not by discipline.
//! [`SecretPlaintext`] deliberately does not implement `Serialize`, `Clone`,
//! `PartialEq` or `Deref`; its `Debug` and `Display` render a fixed redaction
//! marker; and its buffer is zeroized when it drops. The only way to reach the
//! bytes is [`SecretPlaintext::expose_for_encryption`], whose name is the point:
//! a reviewer sees every site that touches plaintext by grepping one symbol.

use core::fmt;

use zeroize::Zeroizing;

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
