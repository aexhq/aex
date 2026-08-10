//! The `regional-secret-custody` key templates and closed vocabularies.
//!
//! Ciphertext never shares a partition with metadata. `SEC#{workspace}` holds
//! one small item per secret name and is what a list query reads; the sealed
//! bytes live under `SECGEN#{workspace}#{name}`, which no list ever touches. The
//! table carries **no stream, no index and no TTL**: a change feed would put
//! secret ciphertext in a consumer role's blast radius, and a timer would be a
//! silent erasure path for material nothing can reconstruct.

use aex_secret_domain::custody::CustodyRevision;
use aex_secret_domain::secret::SourceGeneration;
use aex_session_dynamodb::component::{Component, KeyError, sequence};
use aex_wire::ids::{OperationId, ProviderCredentialId, SessionId, WorkspaceId};

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// Every `itemType` this table may hold, as declared in
/// `migrations/regional/tables/regional-secret-custody.json`.
///
/// The list is closed and is pinned **element-for-element and in order** to the
/// migration by `conformance::the_item_type_vocabulary_equals_the_generation_definition`,
/// so a new member lands as one atomic change across `keys.rs`, the table
/// definition, and the regenerated `migrations/regional/generated/regional-tables.json`
/// — or it does not compile.
pub const ITEM_TYPES: &[&str] = &[
    "workspace_secret",
    "secret_lineage",
    "secret_source_generation",
    "session_custody",
    "custody_binding",
    "managed_call",
    "call_authorization",
    "rebind_intent",
    "provider_credential",
    "idempotency_receipt",
    "custody_counter",
];

/// The `itemType` of a per-workspace quota counter (D-13).
///
/// One type covers both counters. The row shape is identical — a bounded
/// `count` under a `COUNT` sort key — and the partition already says which
/// collection is being counted, so a second discriminator would carry no
/// information. Reusing an *existing* type would have been the wrong economy:
/// that makes the discriminator lie about what the row is.
pub const CUSTODY_COUNTER: &str = "custody_counter";

/// The sort key every quota counter shares.
///
/// It is deliberately outside both listing prefixes — `NAME#` for secrets,
/// `CRED#` for credentials — so a counter can never appear in a customer's page
/// and the listings need no filter to exclude it.
pub const COUNTER_SORT_KEY: &str = "COUNT";

/// Every workspace-secret state, as the domain spells them.
pub const SECRET_STATES: &[&str] = &["ready", "revoked", "deleted"];

/// Every session-custody state, as the domain spells them.
pub const CUSTODY_STATES: &[&str] = &["active", "deleted"];

/// Every provider-credential state, as the wire spells them.
pub const CREDENTIAL_STATES: &[&str] = &["ready", "revoked"];

/// Every BYOK provider, as the generated `ProviderId` spells them.
///
/// Derived from the generated enum rather than written out, so a ninth
/// provider cannot be admitted here without appearing in the contract first.
#[must_use]
pub fn providers() -> Vec<&'static str> {
    aex_wire::models::ProviderId::ALL
        .iter()
        .map(|provider| provider.as_str())
        .collect()
}

/// The metadata partition of one workspace.
#[must_use]
pub fn secret_partition(workspace: WorkspaceId) -> String {
    format!("SEC#{workspace}")
}

/// One workspace secret's metadata. Never carries ciphertext.
///
/// # Errors
///
/// [`KeyError`] when the name could not enter a key.
pub fn secret(workspace: WorkspaceId, name: &str) -> Result<Key, KeyError> {
    let name = Component::parse(name)?;
    Ok(Key {
        pk: secret_partition(workspace),
        sk: format!("NAME#{name}"),
    })
}

/// The sort-key prefix a list-secrets query walks.
#[must_use]
pub const fn secret_prefix() -> &'static str {
    "NAME#"
}

/// One lineage index entry, used only by the audit sweep and never as a fence.
///
/// # Errors
///
/// As [`secret`].
pub fn lineage(
    workspace: WorkspaceId,
    name: &str,
    generation: SourceGeneration,
) -> Result<Key, KeyError> {
    let name = Component::parse(name)?;
    Ok(Key {
        pk: secret_partition(workspace),
        sk: format!("GEN#{name}#{}", sequence(generation.0)),
    })
}

/// The ciphertext partition of one `(workspace, name)`.
///
/// # Errors
///
/// As [`secret`].
pub fn generation_partition(workspace: WorkspaceId, name: &str) -> Result<String, KeyError> {
    let name = Component::parse(name)?;
    Ok(format!("SECGEN#{workspace}#{name}"))
}

/// One hidden source generation, which is the only row that holds ciphertext.
///
/// # Errors
///
/// As [`secret`].
pub fn generation(
    workspace: WorkspaceId,
    name: &str,
    generation: SourceGeneration,
) -> Result<Key, KeyError> {
    Ok(Key {
        pk: generation_partition(workspace, name)?,
        sk: format!("GEN#{}", sequence(generation.0)),
    })
}

/// One session's custody head.
#[must_use]
pub fn custody_head(session: SessionId) -> Key {
    Key {
        pk: custody_partition(session),
        sk: "HEAD".to_owned(),
    }
}

/// The partition every custody row of one session shares.
#[must_use]
pub fn custody_partition(session: SessionId) -> String {
    format!("CUSTODY#{session}")
}

/// One custody binding at one revision.
///
/// Old revisions are retained until purge, so an in-flight run never observes a
/// partial credential change; the current revision is the one on the head.
///
/// # Errors
///
/// As [`secret`].
pub fn binding(session: SessionId, revision: CustodyRevision, name: &str) -> Result<Key, KeyError> {
    let name = Component::parse(name)?;
    Ok(Key {
        pk: custody_partition(session),
        sk: format!("BIND#{}#{name}", sequence(revision.0)),
    })
}

/// The sort-key prefix every binding of one revision shares.
#[must_use]
pub fn binding_prefix(revision: CustodyRevision) -> String {
    format!("BIND#{}#", sequence(revision.0))
}

/// One managed call.
///
/// # Errors
///
/// [`KeyError`] when the call identity could not enter a key.
pub fn managed_call(session: SessionId, call_id: &str) -> Result<Key, KeyError> {
    let call = Component::parse(call_id)?;
    Ok(Key {
        pk: custody_partition(session),
        sk: format!("CALL#{call}"),
    })
}

/// One durable managed-call authorization, the only gate before a decrypt.
///
/// # Errors
///
/// As [`managed_call`].
pub fn authorization(session: SessionId, authorization_id: &str) -> Result<Key, KeyError> {
    let authorization = Component::parse(authorization_id)?;
    Ok(Key {
        pk: custody_partition(session),
        sk: format!("AUTH#{authorization}"),
    })
}

/// One rebind intent.
#[must_use]
pub fn rebind(session: SessionId, operation: OperationId) -> Key {
    Key {
        pk: custody_partition(session),
        sk: format!("REBIND#{operation}"),
    }
}

/// One provider-credential binding in the BYOK directory (OD-23).
///
/// # Errors
///
/// [`KeyError`] when the provider could not enter a key.
pub fn provider_credential(
    workspace: WorkspaceId,
    provider: &str,
    credential: ProviderCredentialId,
) -> Result<Key, KeyError> {
    let provider = Component::parse(provider)?;
    Ok(Key {
        pk: format!("PCR#{workspace}"),
        sk: format!("CRED#{provider}#{credential}"),
    })
}

/// The sort-key prefix a list-credentials query walks.
#[must_use]
pub const fn provider_credential_prefix() -> &'static str {
    "CRED#"
}

/// The partition every provider-credential binding of one workspace shares.
#[must_use]
pub fn provider_credential_partition(workspace: WorkspaceId) -> String {
    format!("PCR#{workspace}")
}

/// One workspace's secret-count quota row (D-13).
///
/// It shares the metadata partition, so the guard is a participant of the same
/// transaction as the write it bounds rather than a second round trip, and it
/// sorts outside `NAME#` so no listing reads it.
#[must_use]
pub fn secret_counter(workspace: WorkspaceId) -> Key {
    Key {
        pk: secret_partition(workspace),
        sk: COUNTER_SORT_KEY.to_owned(),
    }
}

/// One workspace's provider-credential-count quota row (D-13).
#[must_use]
pub fn provider_credential_counter(workspace: WorkspaceId) -> Key {
    Key {
        pk: provider_credential_partition(workspace),
        sk: COUNTER_SORT_KEY.to_owned(),
    }
}

/// One idempotency receipt.
///
/// Delegates to [`aex_session_dynamodb::replay::receipt_key`]: the receipt row
/// shape is shared by every regional table that holds one, and a second key
/// template here would be a second idempotency guarantee.
///
/// # Errors
///
/// [`KeyError`] when the scope or the key digest could not enter a key.
pub fn receipt(workspace: WorkspaceId, scope: &str, key_sha256_hex: &str) -> Result<Key, KeyError> {
    let (pk, sk) = aex_session_dynamodb::replay::receipt_key(workspace, scope, key_sha256_hex)?;
    Ok(Key { pk, sk })
}

#[cfg(test)]
mod tests {
    use aex_secret_domain::custody::CustodyRevision;
    use aex_secret_domain::secret::SourceGeneration;
    use aex_wire::ids::{PrefixedId, SessionId, Uuid7, WorkspaceId};

    use super::{
        binding, custody_head, generation, generation_partition, secret, secret_partition,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn session() -> SessionId {
        SessionId::from_uuid7(Uuid7::compose(1_754_051_696_789, [3; 10]))
    }

    #[test]
    fn ciphertext_never_shares_a_partition_with_the_metadata_a_list_reads() {
        let metadata = secret(workspace(), "openai-key").expect("a key");
        let sealed = generation(workspace(), "openai-key", SourceGeneration::FIRST).expect("a key");
        assert_eq!(metadata.pk, secret_partition(workspace()));
        assert_ne!(
            metadata.pk, sealed.pk,
            "a list-secrets query must not be able to read a ciphertext row"
        );
        assert_eq!(
            sealed.pk,
            generation_partition(workspace(), "openai-key").expect("a partition")
        );
    }

    #[test]
    fn a_generation_sorts_lexicographically_as_it_sorts_numerically() {
        let ninth = generation(workspace(), "k", SourceGeneration(9)).expect("a key");
        let tenth = generation(workspace(), "k", SourceGeneration(10)).expect("a key");
        assert!(ninth.sk < tenth.sk);
    }

    #[test]
    fn a_binding_sorts_by_custody_revision_then_by_name() {
        let first = binding(session(), CustodyRevision(9), "b").expect("a key");
        let second = binding(session(), CustodyRevision(10), "a").expect("a key");
        assert!(first.sk < second.sk);
        assert_eq!(first.pk, custody_head(session()).pk);
    }

    #[test]
    fn a_name_carrying_the_separator_can_never_reach_a_key() {
        assert!(secret(workspace(), "a#b").is_err());
        assert!(generation(workspace(), "a#b", SourceGeneration::FIRST).is_err());
        assert!(binding(session(), CustodyRevision(1), "a#b").is_err());
    }

    /// D-13's counters live in the collections they bound, so the guard is one
    /// participant of the same transaction — but they must sort outside the
    /// prefix a listing walks, or a customer's page would carry a counter row
    /// that decodes as neither a secret nor a credential.
    #[test]
    fn a_quota_counter_shares_its_collection_partition_and_no_listing_prefix() {
        let secrets = super::secret_counter(workspace());
        assert_eq!(secrets.pk, secret_partition(workspace()));
        assert!(!secrets.sk.starts_with(super::secret_prefix()));

        let credentials = super::provider_credential_counter(workspace());
        assert_eq!(
            credentials.pk,
            super::provider_credential_partition(workspace())
        );
        assert!(
            !credentials
                .sk
                .starts_with(super::provider_credential_prefix())
        );
        assert_eq!(secrets.sk, credentials.sk);
        assert_ne!(secrets.pk, credentials.pk);
    }

    #[test]
    fn the_counter_item_type_is_a_declared_member_of_the_closed_vocabulary() {
        assert!(super::ITEM_TYPES.contains(&super::CUSTODY_COUNTER));
    }
}
