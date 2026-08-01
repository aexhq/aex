//! The `regional-registry` key templates and closed vocabularies.
//!
//! This table has **no index and no stream** (D-22). List-by-kind is a native
//! `Query` over one partition, and upload expiry rides the sharded
//! `regional-work` due index rather than the literal single
//! `"registry-upload-expiry"` partition the previous implementation swept.

use aex_content_domain::identity::RegistryKind;
use aex_session_dynamodb::component::{Component, KeyError};
use aex_wire::ids::{UploadId, WorkspaceId};

/// One composite key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The partition key.
    pub pk: String,
    /// The sort key.
    pub sk: String,
}

/// Every `itemType` this table may hold, as declared in
/// `migrations/regional/tables/regional-registry.json`.
pub const ITEM_TYPES: &[&str] = &["registry_pointer", "registry_upload", "idempotency_receipt"];

/// Every upload state, as the domain spells them.
pub const UPLOAD_STATES: &[&str] = &[
    "created",
    "parts_granted",
    "completing",
    "ready",
    "consumed",
    "aborted",
    "expired",
];

/// Every registry kind, as the domain spells them.
pub const KINDS: &[&str] = &["file", "skill", "tool", "instruction", "mcp_server"];

/// The current pointer for one `(workspace, kind, name)`.
///
/// # Errors
///
/// [`KeyError`] when the name could not enter a key. A registered name is an
/// ASCII `ResourceName` by contract, so this can only fire if a caller
/// constructed one by another route.
pub fn pointer(workspace: WorkspaceId, kind: RegistryKind, name: &str) -> Result<Key, KeyError> {
    let name = Component::parse(name)?;
    Ok(Key {
        pk: kind_partition(workspace, kind),
        sk: format!("NAME#{name}"),
    })
}

/// The partition every name of one `(workspace, kind)` shares.
#[must_use]
pub fn kind_partition(workspace: WorkspaceId, kind: RegistryKind) -> String {
    format!("REG#{workspace}#{}", kind.as_str())
}

/// The sort-key prefix a list query walks.
#[must_use]
pub const fn name_prefix() -> &'static str {
    "NAME#"
}

/// One staged upload.
#[must_use]
pub fn upload(upload: UploadId) -> Key {
    Key {
        pk: format!("UPLOAD#{upload}"),
        sk: "STATE".to_owned(),
    }
}

/// One idempotency receipt.
///
/// # Errors
///
/// [`KeyError`] when the rendered scope or the key digest could not enter a key.
pub fn receipt(workspace: WorkspaceId, scope: &str, key_sha256_hex: &str) -> Result<Key, KeyError> {
    let scope = Component::parse(scope)?;
    let digest = Component::parse(key_sha256_hex)?;
    Ok(Key {
        pk: format!("IDEM#{workspace}#{scope}#{digest}"),
        sk: "RECEIPT".to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use aex_content_domain::identity::RegistryKind;
    use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};

    use super::{KINDS, kind_partition, name_prefix, pointer, receipt};

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    #[test]
    fn every_kind_has_its_own_partition_so_a_list_needs_no_filter() {
        let mut seen = std::collections::BTreeSet::new();
        for kind in RegistryKind::ALL {
            seen.insert(kind_partition(workspace(), kind));
        }
        assert_eq!(seen.len(), RegistryKind::ALL.len());
    }

    #[test]
    fn the_kind_vocabulary_matches_the_domain_spelling() {
        let spellings: Vec<&str> = RegistryKind::ALL.iter().map(|kind| kind.as_str()).collect();
        assert_eq!(spellings, KINDS);
    }

    #[test]
    fn a_name_sorts_under_the_prefix_a_list_query_walks() {
        let key = pointer(workspace(), RegistryKind::Tool, "search").expect("a key");
        assert!(key.sk.starts_with(name_prefix()));
        assert_eq!(key.sk, "NAME#search");
    }

    #[test]
    fn a_name_carrying_the_separator_can_never_reach_a_key() {
        assert!(pointer(workspace(), RegistryKind::File, "a#b").is_err());
        assert!(receipt(workspace(), "registry.set:file", &"0".repeat(64)).is_ok());
        assert!(receipt(workspace(), "registry#set", &"0".repeat(64)).is_err());
    }
}
