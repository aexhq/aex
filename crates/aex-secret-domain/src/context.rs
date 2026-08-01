//! The KMS encryption context.
//!
//! Every value is an identifier. Nothing here is derived from plaintext, the key
//! order is fixed rather than map order, and the key set is a closed allowlist —
//! an encryption context is authenticated additional data, so a context that
//! varied by insertion order would make a ciphertext undecryptable by a
//! different build.

use aex_wire::ids::{OrganizationId, WorkspaceId};
use aex_wire::types::Region;

use crate::custody::CustodyRevision;
use crate::secret::{SecretName, SourceGeneration};

/// The closed allowlist of context keys, in the fixed canonical order.
pub const CONTEXT_KEYS: [&str; 7] = [
    "aex:plane",
    "aex:region",
    "aex:organization",
    "aex:workspace",
    "aex:name",
    "aex:generation",
    "aex:custody_revision",
];

/// Which deployment plane a ciphertext belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Plane {
    /// The development plane.
    Dev,
    /// The production plane.
    Prd,
}

impl Plane {
    /// Every plane, in canonical order.
    pub const ALL: [Self; 2] = [Self::Dev, Self::Prd];

    /// The stable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prd => "prd",
        }
    }
}

/// The authenticated context a secret ciphertext is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionContext {
    /// Which plane.
    pub plane: Plane,
    /// Which region.
    pub region: Region,
    /// Which organization.
    pub organization: OrganizationId,
    /// Which workspace.
    pub workspace: WorkspaceId,
    /// Which secret name.
    pub name: SecretName,
    /// Which source generation.
    pub generation: SourceGeneration,
    /// Which custody revision, when the ciphertext is session custody.
    pub custody_revision: Option<CustodyRevision>,
}

impl EncryptionContext {
    /// The context pairs, in the fixed canonical order.
    ///
    /// A pair is omitted only when its value is absent, and the remaining pairs
    /// keep their relative order, so a decryptor rebuilding the context from the
    /// same identifiers always produces the same sequence.
    #[must_use]
    pub fn canonical_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            (CONTEXT_KEYS[0], self.plane.as_str().to_owned()),
            (CONTEXT_KEYS[1], self.region.code().to_owned()),
            (CONTEXT_KEYS[2], self.organization.to_string()),
            (CONTEXT_KEYS[3], self.workspace.to_string()),
            (CONTEXT_KEYS[4], self.name.as_str().to_owned()),
            (CONTEXT_KEYS[5], self.generation.0.to_string()),
        ];
        if let Some(revision) = self.custody_revision {
            pairs.push((CONTEXT_KEYS[6], revision.0.to_string()));
        }
        pairs
    }

    /// A digest over the canonical pairs.
    ///
    /// Hand-written and length-prefixed, so the digest cannot move because a
    /// serializer changed.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"aex.secret.context.v1");
        for (key, value) in self.canonical_pairs() {
            hasher.update(&u32_len(key.len()));
            hasher.update(key.as_bytes());
            hasher.update(&u32_len(value.len()));
            hasher.update(value.as_bytes());
        }
        *hasher.finalize().as_bytes()
    }
}

fn u32_len(value: usize) -> [u8; 4] {
    u32::try_from(value)
        .unwrap_or_else(|_| unreachable!("context values are bounded identifiers"))
        .to_le_bytes()
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{OrganizationId, PrefixedId as _, Uuid7, WorkspaceId};
    use aex_wire::types::Region;

    use super::{CONTEXT_KEYS, EncryptionContext, Plane};
    use crate::custody::CustodyRevision;
    use crate::secret::{SecretName, SourceGeneration};

    fn context() -> EncryptionContext {
        EncryptionContext {
            plane: Plane::Prd,
            region: Region::ALL[0],
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [2; 10])),
            name: SecretName::parse("openai-key").expect("valid"),
            generation: SourceGeneration(3),
            custody_revision: None,
        }
    }

    #[test]
    fn the_key_order_is_fixed_and_from_the_allowlist() {
        let pairs = context().canonical_pairs();
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, CONTEXT_KEYS[..6].to_vec());
        for (key, _) in &pairs {
            assert!(CONTEXT_KEYS.contains(key));
        }

        let with_custody = EncryptionContext {
            custody_revision: Some(CustodyRevision(9)),
            ..context()
        };
        let keys: Vec<&str> = with_custody
            .canonical_pairs()
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(keys, CONTEXT_KEYS.to_vec());
    }

    #[test]
    fn the_digest_moves_with_every_identifier() {
        let base = context().digest();
        assert_eq!(base, context().digest());
        assert_ne!(
            base,
            EncryptionContext {
                generation: SourceGeneration(4),
                ..context()
            }
            .digest()
        );
        assert_ne!(
            base,
            EncryptionContext {
                plane: Plane::Dev,
                ..context()
            }
            .digest()
        );
    }
}
