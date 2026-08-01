//! The provider-mandated branch-key record.
//!
//! This is the one regional table that does **not** use the `pk`/`sk`
//! convention, and the one whose rows carry no `itemType`. Both are deliberate:
//! the schema belongs to the AWS Encryption SDK hierarchical keyring, not to
//! AEX, and a record written by the provider's own key-store operations has to
//! decode here byte for byte. Deviating would produce a key store this code
//! could read and the provider tooling could not.
//!
//! Because the rows carry no discriminator the shared typed row reader does not
//! apply, so this module does its own decoding — narrowly, and only over the
//! attributes the schema declares.

use std::collections::BTreeMap;

use aex_session_dynamodb::attr::Item;
use aex_wire::ids::WorkspaceId;

/// The partition key attribute, as the provider names it.
pub const BRANCH_KEY_ID: &str = "branch-key-id";

/// The sort key attribute, as the provider names it.
pub const TYPE: &str = "type";

/// The wrapped branch-key material.
pub const ENC: &str = "enc";

/// The KMS key the material is wrapped under.
pub const KMS_ARN: &str = "kms-arn";

/// When the record was created, ISO 8601.
pub const CREATE_TIME: &str = "create-time";

/// The hierarchy version.
pub const HIERARCHY_VERSION: &str = "hierarchy-version";

/// The active record's pointer at its versioned record.
pub const VERSION: &str = "version";

/// The prefix the provider persists custom encryption-context pairs under.
pub const CUSTOM_CONTEXT_PREFIX: &str = "aws-crypto-ec:";

/// The sort-key value of the active record.
pub const ACTIVE: &str = "branch:ACTIVE";

/// The sort-key prefix of a versioned record.
pub const VERSION_PREFIX: &str = "branch:version:";

/// The sort-key value of the beacon record.
pub const BEACON_ACTIVE: &str = "beacon:ACTIVE";

/// A branch key identity.
///
/// It **is** the workspace id. One branch key per workspace makes the
/// branch-key-id supplier a pure function of the encryption context — a caller
/// cannot select another tenant's key by naming it — and keeps `CloudTrail` free
/// of anything but an identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BranchKeyId(String);

impl BranchKeyId {
    /// The branch key of one workspace.
    #[must_use]
    pub fn of(workspace: WorkspaceId) -> Self {
        Self(workspace.to_string())
    }

    /// Wraps an identity read back from the store.
    #[must_use]
    pub fn from_stored(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The identity as the provider stores it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchKeyId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Which record a row is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordKind {
    /// The active branch key.
    Active,
    /// One versioned branch key.
    Version(String),
    /// The beacon key.
    Beacon,
}

impl RecordKind {
    /// The sort-key value.
    #[must_use]
    pub fn as_sort_key(&self) -> String {
        match self {
            Self::Active => ACTIVE.to_owned(),
            Self::Version(uuid) => format!("{VERSION_PREFIX}{uuid}"),
            Self::Beacon => BEACON_ACTIVE.to_owned(),
        }
    }

    /// Resolves a stored sort-key value.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            ACTIVE => Some(Self::Active),
            BEACON_ACTIVE => Some(Self::Beacon),
            other => other
                .strip_prefix(VERSION_PREFIX)
                .map(|uuid| Self::Version(uuid.to_owned())),
        }
    }
}

/// Why a stored record could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    /// A required attribute was absent.
    #[error("a branch key record is missing `{attribute}`")]
    Missing {
        /// Which attribute.
        attribute: &'static str,
    },
    /// An attribute carried the wrong type.
    #[error("a branch key record's `{attribute}` is the wrong type")]
    WrongType {
        /// Which attribute.
        attribute: &'static str,
    },
    /// The sort key was outside the provider's vocabulary.
    #[error("`{found}` is not a branch key record type")]
    UnknownType {
        /// What was stored.
        found: String,
    },
}

/// One branch-key record, as the provider writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchKeyRecord {
    /// Which branch key.
    pub branch_key_id: BranchKeyId,
    /// Which record.
    pub kind: RecordKind,
    /// The wrapped material.
    pub enc: Vec<u8>,
    /// The KMS key it is wrapped under.
    pub kms_arn: String,
    /// When it was created.
    pub create_time: String,
    /// The hierarchy version.
    pub hierarchy_version: u64,
    /// The versioned record the active record points at.
    pub version: Option<String>,
    /// The custom encryption-context pairs, without their storage prefix.
    pub custom_context: BTreeMap<String, String>,
}

/// Decodes one stored record.
///
/// # Errors
///
/// [`RecordError`] for a missing attribute, a wrong type, or a record type
/// outside the provider's vocabulary. Nothing is defaulted: a half-read branch
/// key is a key that decrypts nothing.
pub fn decode(item: &Item) -> Result<BranchKeyRecord, RecordError> {
    let text = |attribute: &'static str| -> Result<String, RecordError> {
        item.get(attribute)
            .ok_or(RecordError::Missing { attribute })?
            .as_s()
            .cloned()
            .map_err(|_| RecordError::WrongType { attribute })
    };
    let type_text = text(TYPE)?;
    let kind =
        RecordKind::parse(&type_text).ok_or(RecordError::UnknownType { found: type_text })?;
    let enc = item
        .get(ENC)
        .ok_or(RecordError::Missing { attribute: ENC })?
        .as_b()
        .map(|blob| blob.as_ref().to_vec())
        .map_err(|_| RecordError::WrongType { attribute: ENC })?;
    let hierarchy_version = item
        .get(HIERARCHY_VERSION)
        .ok_or(RecordError::Missing {
            attribute: HIERARCHY_VERSION,
        })?
        .as_n()
        .map_err(|_| RecordError::WrongType {
            attribute: HIERARCHY_VERSION,
        })?
        .parse::<u64>()
        .map_err(|_| RecordError::WrongType {
            attribute: HIERARCHY_VERSION,
        })?;
    let mut custom_context = BTreeMap::new();
    for (name, value) in item {
        if let Some(stripped) = name.strip_prefix(CUSTOM_CONTEXT_PREFIX) {
            let value = value.as_s().map_err(|_| RecordError::WrongType {
                attribute: "a custom context pair",
            })?;
            custom_context.insert(stripped.to_owned(), value.clone());
        }
    }
    Ok(BranchKeyRecord {
        branch_key_id: BranchKeyId::from_stored(text(BRANCH_KEY_ID)?),
        kind,
        enc,
        kms_arn: text(KMS_ARN)?,
        create_time: text(CREATE_TIME)?,
        hierarchy_version,
        version: item
            .get(VERSION)
            .and_then(|value| value.as_s().ok())
            .cloned(),
        custom_context,
    })
}

#[cfg(test)]
mod tests {
    use aex_session_dynamodb::attr::{Item, b, n, s};
    use aex_wire::ids::{PrefixedId, Uuid7, WorkspaceId};

    use super::{
        ACTIVE, BRANCH_KEY_ID, BranchKeyId, CREATE_TIME, CUSTOM_CONTEXT_PREFIX, ENC,
        HIERARCHY_VERSION, KMS_ARN, RecordError, RecordKind, TYPE, VERSION, decode,
    };

    fn workspace() -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1_754_051_696_789, [1; 10]))
    }

    fn record() -> Item {
        let mut item = Item::new();
        item.insert(BRANCH_KEY_ID.to_owned(), s(workspace().to_string()));
        item.insert(TYPE.to_owned(), s(ACTIVE));
        item.insert(ENC.to_owned(), b(vec![0xaa; 64]));
        item.insert(
            KMS_ARN.to_owned(),
            s("arn:aws:kms:eu-west-1:000000000000:key/secret"),
        );
        item.insert(CREATE_TIME.to_owned(), s("2026-08-01T12:34:56.789Z"));
        item.insert(HIERARCHY_VERSION.to_owned(), n(1));
        item.insert(VERSION.to_owned(), s("branch:version:abc"));
        item.insert(format!("{CUSTOM_CONTEXT_PREFIX}aex:plane"), s("prd"));
        item
    }

    #[test]
    fn the_branch_key_identity_is_the_workspace_identity() {
        assert_eq!(
            BranchKeyId::of(workspace()).as_str(),
            workspace().to_string()
        );
    }

    #[test]
    fn an_active_record_decodes_with_its_pointer_and_its_custom_context() {
        let decoded = decode(&record()).expect("decodes");
        assert_eq!(decoded.kind, RecordKind::Active);
        assert_eq!(decoded.version.as_deref(), Some("branch:version:abc"));
        assert_eq!(decoded.custom_context["aex:plane"], "prd");
        assert_eq!(decoded.hierarchy_version, 1);
    }

    #[test]
    fn every_record_type_round_trips_through_its_sort_key() {
        for kind in [
            RecordKind::Active,
            RecordKind::Version("abc".to_owned()),
            RecordKind::Beacon,
        ] {
            assert_eq!(RecordKind::parse(&kind.as_sort_key()), Some(kind));
        }
        assert_eq!(RecordKind::parse("branch:teleport"), None);
    }

    #[test]
    fn a_record_missing_its_material_is_refused_rather_than_defaulted() {
        let mut broken = record();
        broken.remove(ENC);
        assert_eq!(
            decode(&broken).unwrap_err(),
            RecordError::Missing { attribute: ENC }
        );
    }

    #[test]
    fn a_record_type_outside_the_provider_vocabulary_is_refused() {
        let mut broken = record();
        broken.insert(TYPE.to_owned(), s("branch:teleport"));
        assert!(matches!(
            decode(&broken).unwrap_err(),
            RecordError::UnknownType { .. }
        ));
    }
}
