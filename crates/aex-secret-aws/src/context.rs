//! The KMS-visible encryption context, and the one place the secret name is
//! turned into a digest.
//!
//! `aex_secret_domain::EncryptionContext` is the logical binding and carries
//! `aex:name` in the clear, which is right for an in-process identity. The
//! context this module renders is the one that reaches **KMS**, and it therefore
//! carries `aex:name-digest` instead: an encryption context is authenticated but
//! it is also recorded in `CloudTrail`, and a customer-chosen secret name has no
//! business being written into an audit log the customer cannot redact (D-17).
//!
//! Everything else is identifiers only, for the same reason.

use aex_secret_domain::context::EncryptionContext;
use std::collections::BTreeMap;

/// The context key carrying the digest of the secret name.
pub const NAME_DIGEST_KEY: &str = "aex:name-digest";

/// The context key the domain uses for the name in the clear. It never reaches
/// KMS.
pub const NAME_KEY: &str = "aex:name";

/// The context key naming the plane a branch key belongs to.
pub const PLANE_KEY: &str = "aex:plane";

/// The context key naming the region.
pub const REGION_KEY: &str = "aex:region";

/// The context key naming the workspace, which **is** the branch key identity.
pub const WORKSPACE_KEY: &str = "aex:workspace";

/// The **branch key's** own encryption context: plane, region and workspace.
///
/// This is what wraps and unwraps branch material at `KMS`, and it is
/// deliberately narrower than [`kms_pairs`].
///
/// # Why it is not the secret's context
///
/// A branch key is per **workspace** (`BranchKeyId::of`), so it is wrapped once
/// and opened by every secret that workspace ever holds. An encryption context
/// is authenticated additional data: whatever wraps a value must be reproducible
/// **exactly** by every future opener, from information the opener has. The
/// secret's context carries `aex:organization`, `aex:name-digest`,
/// `aex:generation` and sometimes `aex:custody_revision` — none of which the
/// workspace's single branch key can be bound to, because the same key must open
/// every name and every generation. Wrapping under the secret's context would
/// require one wrapped key per `(name, generation)`, which is exactly the
/// per-write `GenerateDataKey` the branch-key hierarchy exists to avoid: a `KMS`
/// round trip on every write *and every open*, and a cache that can never hit.
///
/// # What still binds a value to its exact identity
///
/// Everything the narrower context drops is enforced one layer down, and more
/// strongly: the **full** secret context is the AEAD additional data
/// ([`crate::context::aad_bytes`]) and its digest is stored beside the
/// ciphertext, so a frame moved between organizations, names, generations or
/// custody revisions fails to open — and the digest comparison catches it
/// **before** a `KMS` call is spent. Tenant isolation is unweakened and remains
/// enforced at both layers: `aex:workspace` is in this context, so `KMS` itself
/// refuses to open one tenant's branch key while another tenant is claimed.
///
/// The rotation attestation and the rotation-reason digest are deliberately
/// **not** here. They are recorded on the branch-key row for audit, but they are
/// not reproducible by an opener that only knows which workspace it is serving,
/// and an encryption context nobody can rebuild is a key nobody can use.
#[must_use]
pub fn branch_key_pairs(context: &EncryptionContext) -> BTreeMap<String, String> {
    branch_key_context(
        context.plane.as_str(),
        context.region.code(),
        &context.workspace.to_string(),
    )
}

/// The same context, built from the three identifiers directly.
///
/// The provisioning path has no [`EncryptionContext`] — it is creating the key a
/// context will later be built against — so it composes the pairs from the plane,
/// the region and the branch key id. One function, so a wrap and an unwrap
/// cannot disagree about the spelling.
#[must_use]
pub fn branch_key_context(plane: &str, region: &str, workspace: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (PLANE_KEY.to_owned(), plane.to_owned()),
        (REGION_KEY.to_owned(), region.to_owned()),
        (WORKSPACE_KEY.to_owned(), workspace.to_owned()),
    ])
}

/// Renders the KMS-visible pairs, in canonical order.
///
/// The pairs are a `BTreeMap`, so the order a decryptor rebuilds is the order an
/// encryptor sent whatever the caller does.
#[must_use]
pub fn kms_pairs(context: &EncryptionContext) -> BTreeMap<String, String> {
    let mut pairs = BTreeMap::new();
    for (key, value) in context.canonical_pairs() {
        if key == NAME_KEY {
            pairs.insert(
                NAME_DIGEST_KEY.to_owned(),
                name_digest(&context.workspace.to_string(), &value),
            );
        } else {
            pairs.insert(key.to_owned(), value);
        }
    }
    pairs
}

/// The digest that stands in for a secret name.
///
/// Salted with the workspace so the same name in two workspaces produces two
/// digests, which stops a `CloudTrail` reader from correlating tenants by their
/// naming conventions.
#[must_use]
pub fn name_digest(workspace: &str, name: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(workspace.as_bytes());
    hasher.update([0x00]);
    hasher.update(name.as_bytes());
    hex::encode(hasher.finalize())
}

/// The canonical byte encoding of the KMS context, used as AEAD additional
/// authenticated data.
///
/// Length-prefixed by hand rather than serialized, so the bytes cannot move
/// because a serializer changed. Sealing binds the ciphertext to exactly these
/// bytes: opening under any other context fails at the tag rather than
/// returning something plausible.
#[must_use]
pub fn aad_bytes(context: &EncryptionContext) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(b"aex.secret.kms-context.v1");
    for (key, value) in kms_pairs(context) {
        bytes.extend_from_slice(&u32_len(key.len()));
        bytes.extend_from_slice(key.as_bytes());
        bytes.extend_from_slice(&u32_len(value.len()));
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

/// The digest stored beside every ciphertext, so a context mismatch is caught
/// before a KMS call is spent.
#[must_use]
pub fn context_digest(context: &EncryptionContext) -> [u8; 32] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(aad_bytes(context));
    hasher.finalize().into()
}

fn u32_len(value: usize) -> [u8; 4] {
    u32::try_from(value)
        .unwrap_or_else(|_| unreachable!("context values are bounded identifiers"))
        .to_le_bytes()
}

#[cfg(test)]
mod tests {
    use aex_secret_domain::context::{EncryptionContext, Plane};
    use aex_secret_domain::custody::CustodyRevision;
    use aex_secret_domain::secret::{SecretName, SourceGeneration};
    use aex_wire::ids::{OrganizationId, PrefixedId, Uuid7, WorkspaceId};
    use aex_wire::types::Region;

    use super::{NAME_DIGEST_KEY, NAME_KEY, aad_bytes, context_digest, kms_pairs};

    fn context(name: &str, workspace_byte: u8) -> EncryptionContext {
        EncryptionContext {
            plane: Plane::Prd,
            region: Region::ALL[0],
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [1; 10])),
            workspace: WorkspaceId::from_uuid7(Uuid7::compose(1, [workspace_byte; 10])),
            name: SecretName::parse(name).expect("an ASCII name"),
            generation: SourceGeneration(3),
            custody_revision: Some(CustodyRevision(2)),
        }
    }

    #[test]
    fn the_customer_chosen_name_never_reaches_the_context_that_reaches_cloudtrail() {
        let pairs = kms_pairs(&context("acme-production-stripe", 1));
        assert!(
            !pairs.contains_key(NAME_KEY),
            "an encryption context is recorded in CloudTrail: {pairs:?}"
        );
        let digest = pairs.get(NAME_DIGEST_KEY).expect("a digest");
        assert_eq!(digest.len(), 64);
        assert!(
            !pairs
                .values()
                .any(|value| value.contains("acme-production-stripe"))
        );
    }

    #[test]
    fn the_same_name_in_two_workspaces_digests_differently() {
        let mine = kms_pairs(&context("openai-key", 1));
        let theirs = kms_pairs(&context("openai-key", 2));
        assert_ne!(mine[NAME_DIGEST_KEY], theirs[NAME_DIGEST_KEY]);
    }

    #[test]
    fn the_aad_is_stable_and_moves_when_any_field_moves() {
        let first = aad_bytes(&context("openai-key", 1));
        assert_eq!(first, aad_bytes(&context("openai-key", 1)));
        assert_ne!(first, aad_bytes(&context("openai-key", 2)));
        assert_ne!(first, aad_bytes(&context("other-key", 1)));

        let mut later = context("openai-key", 1);
        later.generation = SourceGeneration(4);
        assert_ne!(first, aad_bytes(&later));
    }

    #[test]
    fn the_stored_digest_is_a_pure_function_of_the_aad() {
        let bound = context("openai-key", 1);
        assert_eq!(context_digest(&bound), context_digest(&bound));
        assert_ne!(context_digest(&bound), context_digest(&context("k", 1)));
    }
}
