//! `regional-secret-custody` row codecs.
//!
//! One rule shapes every type here: **a metadata row can never carry
//! ciphertext**. The list-secrets path reads [`SecretMetadata`], which has
//! nowhere to put sealed bytes, and the sealed bytes live on a row in another
//! partition that only the unwrap path reads.

use crate::keys;
use aex_secret_domain::custody::{CustodyEntry, CustodyRevision, CustodyState, OwnerKeyEdgeId};
use aex_secret_domain::revocation::RevocationEpoch;
use aex_secret_domain::secret::{
    CiphertextRef, SecretName, SecretRevision, SecretState, SourceGeneration,
};
use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, PK, Row, SK, b, n, s, stamp};
use aex_session_dynamodb::component::KeyError;
use aex_wire::ids::{
    ContentHash, ProviderCredentialId, ResourceName, SessionId, Uuid7, WorkspaceId,
};
use aex_wire::models::ProviderId;
use aex_wire::types::Timestamp;

/// The `itemType` of one workspace secret's metadata.
pub const WORKSPACE_SECRET: &str = "workspace_secret";
/// The `itemType` of one lineage index entry.
pub const SECRET_LINEAGE: &str = "secret_lineage";
/// The `itemType` of one hidden source generation.
pub const SECRET_SOURCE_GENERATION: &str = "secret_source_generation";
/// The `itemType` of one session custody head.
pub const SESSION_CUSTODY: &str = "session_custody";
/// The `itemType` of one custody binding.
pub const CUSTODY_BINDING: &str = "custody_binding";
/// The `itemType` of one managed-call authorization.
pub const CALL_AUTHORIZATION: &str = "call_authorization";
/// The `itemType` of one provider-credential binding.
pub const PROVIDER_CREDENTIAL: &str = "provider_credential";

/// Why a row could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A key component was unusable.
    #[error(transparent)]
    Key(#[from] KeyError),
}

/// One workspace secret, without its ciphertext.
///
/// This is what a list reads and what every fence conditions on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMetadata {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The name.
    pub name: SecretName,
    /// The current source generation.
    pub generation: SourceGeneration,
    /// The current record revision.
    pub revision: SecretRevision,
    /// Where the record is.
    pub state: SecretState,
    /// The revocation epoch in force.
    pub revocation_epoch: RevocationEpoch,
    /// Every generation at or below this revision is denied.
    pub revoked_through_revision: SecretRevision,
    /// When the record was created.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When it was revoked.
    pub revoked_at: Option<Timestamp>,
}

/// The stable wire spelling of a secret state.
#[must_use]
pub const fn secret_state_str(state: SecretState) -> &'static str {
    match state {
        SecretState::Ready => "ready",
        SecretState::Revoked => "revoked",
        SecretState::Deleted => "deleted",
    }
}

/// Resolves a secret state. There is no alias table.
#[must_use]
pub fn secret_state_of(text: &str) -> Option<SecretState> {
    SecretState::ALL
        .into_iter()
        .find(|state| secret_state_str(*state) == text)
}

/// The stable wire spelling of a custody state.
#[must_use]
pub const fn custody_state_str(state: CustodyState) -> &'static str {
    match state {
        CustodyState::Active => "active",
        CustodyState::Deleted => "deleted",
    }
}

/// Resolves a custody state.
#[must_use]
pub const fn custody_state_of(text: &str) -> Option<CustodyState> {
    match text.as_bytes() {
        b"active" => Some(CustodyState::Active),
        b"deleted" => Some(CustodyState::Deleted),
        _ => None,
    }
}

/// Encodes one secret's metadata.
///
/// # Errors
///
/// [`EncodeError`] when the name could not enter a key.
pub fn encode_secret(secret: &SecretMetadata) -> Result<Item, EncodeError> {
    let key = keys::secret(secret.workspace, secret.name.as_str())?;
    Ok(ItemBuilder::new(WORKSPACE_SECRET)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(secret.workspace.to_string()))
        .set("name", s(secret.name.as_str().to_owned()))
        .set("activeSourceGeneration", n(secret.generation.0))
        .set("revision", n(secret.revision.0))
        .set("state", s(secret_state_str(secret.state)))
        .set("revocationEpoch", n(secret.revocation_epoch.0))
        .set(
            "revokedThroughRevision",
            n(secret.revoked_through_revision.0),
        )
        .set("createdAt", stamp(secret.created_at))
        .set("updatedAt", stamp(secret.updated_at))
        .set_opt("revokedAt", secret.revoked_at.map(stamp))
        .build())
}

/// Decodes one secret's metadata and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] as for every decode here, plus a hard refusal of a metadata
/// row that carries sealed bytes: that is corruption, and reading it would put
/// secret material on the list path.
pub fn decode_secret(item: &Item, asserted: WorkspaceId) -> Result<SecretMetadata, CodecError> {
    let row = Row::bind(item, WORKSPACE_SECRET)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    refuse_sealed(
        item,
        WORKSPACE_SECRET,
        "a metadata row must never carry sealed bytes",
    )?;
    Ok(SecretMetadata {
        workspace: asserted,
        name: name_of(&row, WORKSPACE_SECRET)?,
        generation: SourceGeneration(row.u64("activeSourceGeneration")?),
        revision: SecretRevision(row.u64("revision")?),
        state: secret_state_of(row.enumerated("state", keys::SECRET_STATES)?).ok_or(
            CodecError::Malformed {
                item_type: WORKSPACE_SECRET,
                attribute: "state",
                reason: "outside the secret state vocabulary".to_owned(),
            },
        )?,
        revocation_epoch: RevocationEpoch(row.u64("revocationEpoch")?),
        revoked_through_revision: SecretRevision(row.u64("revokedThroughRevision")?),
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
        revoked_at: row.opt_timestamp("revokedAt")?,
    })
}

/// One hidden source generation: the only row that holds ciphertext.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredGeneration {
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The name.
    pub name: SecretName,
    /// Which generation.
    pub generation: SourceGeneration,
    /// The sealed value.
    pub ciphertext: CiphertextRef,
    /// The digest of the context the value is bound to.
    pub context_digest: [u8; 32],
    /// When it was minted.
    pub created_at: Timestamp,
    /// When the lazy audit sweep marked it revoked.
    pub revoked_at: Option<Timestamp>,
}

/// Encodes one hidden source generation.
///
/// # Errors
///
/// [`EncodeError`] when the name could not enter a key.
pub fn encode_generation(generation: &StoredGeneration) -> Result<Item, EncodeError> {
    let key = keys::generation(
        generation.workspace,
        generation.name.as_str(),
        generation.generation,
    )?;
    Ok(ItemBuilder::new(SECRET_SOURCE_GENERATION)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(generation.workspace.to_string()))
        .set("name", s(generation.name.as_str().to_owned()))
        .set("sourceGeneration", n(generation.generation.0))
        .set("keyGeneration", n(generation.ciphertext.key_generation))
        .set("wrappedKey", b(generation.ciphertext.wrapped_key.clone()))
        .set("nonce", b(generation.ciphertext.nonce.clone()))
        .set("ciphertext", b(generation.ciphertext.ciphertext.clone()))
        .set(
            "encContextDigest",
            s(hex::encode(generation.context_digest)),
        )
        .set("createdAt", stamp(generation.created_at))
        .set_opt("revokedAt", generation.revoked_at.map(stamp))
        .build())
}

/// Decodes one hidden source generation.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_generation(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<StoredGeneration, CodecError> {
    let row = Row::bind(item, SECRET_SOURCE_GENERATION)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(StoredGeneration {
        workspace: asserted,
        name: name_of(&row, SECRET_SOURCE_GENERATION)?,
        generation: SourceGeneration(row.u64("sourceGeneration")?),
        ciphertext: CiphertextRef {
            key_generation: row.u64("keyGeneration")?,
            wrapped_key: row.bytes("wrappedKey")?.to_vec(),
            nonce: row.bytes("nonce")?.to_vec(),
            ciphertext: row.bytes("ciphertext")?.to_vec(),
        },
        context_digest: digest_of(&row, SECRET_SOURCE_GENERATION, "encContextDigest")?,
        created_at: row.timestamp("createdAt")?,
        revoked_at: row.opt_timestamp("revokedAt")?,
    })
}

/// Encodes one lineage index entry, which carries no ciphertext and is never a
/// fence.
///
/// # Errors
///
/// [`EncodeError`] when the name could not enter a key.
pub fn encode_lineage(
    workspace: WorkspaceId,
    name: &SecretName,
    generation: SourceGeneration,
    created_at: Timestamp,
) -> Result<Item, EncodeError> {
    let key = keys::lineage(workspace, name.as_str(), generation)?;
    Ok(ItemBuilder::new(SECRET_LINEAGE)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("workspaceId", s(workspace.to_string()))
        .set("name", s(name.as_str().to_owned()))
        .set("sourceGeneration", n(generation.0))
        .set("createdAt", stamp(created_at))
        .build())
}

/// One session's custody head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyHead {
    /// The session.
    pub session: SessionId,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The current revision.
    pub revision: CustodyRevision,
    /// The key edge every authorization at this revision derives from.
    pub owner_key_edge: OwnerKeyEdgeId,
    /// Where the row is.
    pub state: CustodyState,
    /// The idle epoch a rebind conditions on.
    pub idle_epoch: u64,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// One immutable session binding at an exact custody revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustodyBinding {
    /// The session whose custody owns the binding.
    pub session: SessionId,
    /// The workspace whose source secret was admitted.
    pub workspace: WorkspaceId,
    /// The custody revision at which the binding was written.
    pub revision: CustodyRevision,
    /// The admitted secret and its session-scoped ciphertext.
    pub entry: CustodyEntry,
    /// The authenticated encryption-context digest stored beside the ciphertext.
    pub context_digest: [u8; 32],
}

/// Encodes one custody head.
#[must_use]
pub fn encode_custody_head(head: &CustodyHead) -> Item {
    let key = keys::custody_head(head.session);
    ItemBuilder::new(SESSION_CUSTODY)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(head.session.to_string()))
        .set("workspaceId", s(head.workspace.to_string()))
        .set("custodyRevision", n(head.revision.0))
        .set(
            "ownerKeyEdgeId",
            s(hex::encode(head.owner_key_edge.0.as_bytes())),
        )
        .set("state", s(custody_state_str(head.state)))
        .set("idleEpoch", n(head.idle_epoch))
        .set("updatedAt", stamp(head.updated_at))
        .build()
}

/// Decodes one custody head.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_custody_head(item: &Item, asserted: WorkspaceId) -> Result<CustodyHead, CodecError> {
    let row = Row::bind(item, SESSION_CUSTODY)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(CustodyHead {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        revision: CustodyRevision(row.u64("custodyRevision")?),
        owner_key_edge: OwnerKeyEdgeId(uuid_of(&row, "ownerKeyEdgeId")?),
        state: custody_state_of(row.enumerated("state", keys::CUSTODY_STATES)?).ok_or(
            CodecError::Malformed {
                item_type: SESSION_CUSTODY,
                attribute: "state",
                reason: "outside the custody state vocabulary".to_owned(),
            },
        )?,
        idle_epoch: row.u64("idleEpoch")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// Encodes one custody binding at one revision.
///
/// # Errors
///
/// [`EncodeError`] when the name could not enter a key.
pub fn encode_binding(
    session: SessionId,
    workspace: WorkspaceId,
    revision: CustodyRevision,
    entry: &CustodyEntry,
    context_digest: [u8; 32],
    created_at: Timestamp,
) -> Result<Item, EncodeError> {
    let key = keys::binding(session, revision, entry.name.as_str())?;
    Ok(ItemBuilder::new(CUSTODY_BINDING)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(session.to_string()))
        .set("workspaceId", s(workspace.to_string()))
        .set("custodyRevision", n(revision.0))
        .set("name", s(entry.name.as_str().to_owned()))
        .set("sourceGeneration", n(entry.source_generation.0))
        .set("sourceRevision", n(entry.source_revision.0))
        .set("epochAtAdmission", n(entry.epoch_at_admission.0))
        .set("keyGeneration", n(entry.ciphertext.key_generation))
        .set("wrappedKey", b(entry.ciphertext.wrapped_key.clone()))
        .set("nonce", b(entry.ciphertext.nonce.clone()))
        .set("ciphertext", b(entry.ciphertext.ciphertext.clone()))
        .set("encContextDigest", s(hex::encode(context_digest)))
        .set("createdAt", stamp(created_at))
        .build())
}

/// Decodes one custody binding.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_binding(item: &Item, asserted: WorkspaceId) -> Result<CustodyBinding, CodecError> {
    let row = Row::bind(item, CUSTODY_BINDING)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(CustodyBinding {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        revision: CustodyRevision(row.u64("custodyRevision")?),
        entry: CustodyEntry {
            name: name_of(&row, CUSTODY_BINDING)?,
            source_generation: SourceGeneration(row.u64("sourceGeneration")?),
            source_revision: SecretRevision(row.u64("sourceRevision")?),
            epoch_at_admission: RevocationEpoch(row.u64("epochAtAdmission")?),
            ciphertext: CiphertextRef {
                key_generation: row.u64("keyGeneration")?,
                wrapped_key: row.bytes("wrappedKey")?.to_vec(),
                nonce: row.bytes("nonce")?.to_vec(),
                ciphertext: row.bytes("ciphertext")?.to_vec(),
            },
        },
        context_digest: digest_of(&row, CUSTODY_BINDING, "encContextDigest")?,
    })
}

/// One durable managed-call authorization. Immutable once written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallAuthorization {
    /// Its identity.
    pub authorization_id: String,
    /// The session whose custody it derives from.
    pub session: SessionId,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// Which name it authorises.
    pub name: SecretName,
    /// The custody revision it was taken at.
    pub custody_revision: CustodyRevision,
    /// The source generation it is bound to.
    pub source_generation: SourceGeneration,
    /// The key edge it derives from.
    pub owner_key_edge: OwnerKeyEdgeId,
    /// When it was granted.
    pub created_at: Timestamp,
}

/// Encodes one managed-call authorization.
///
/// # Errors
///
/// [`EncodeError`] when the identity could not enter a key.
pub fn encode_authorization(authorization: &CallAuthorization) -> Result<Item, EncodeError> {
    let key = keys::authorization(authorization.session, &authorization.authorization_id)?;
    Ok(ItemBuilder::new(CALL_AUTHORIZATION)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("authorizationId", s(authorization.authorization_id.clone()))
        .set("sessionId", s(authorization.session.to_string()))
        .set("workspaceId", s(authorization.workspace.to_string()))
        .set("name", s(authorization.name.as_str().to_owned()))
        .set("custodyRevision", n(authorization.custody_revision.0))
        .set("sourceGeneration", n(authorization.source_generation.0))
        .set(
            "ownerKeyEdgeId",
            s(hex::encode(authorization.owner_key_edge.0.as_bytes())),
        )
        .set("createdAt", stamp(authorization.created_at))
        .build())
}

/// Whether a BYOK binding may still be selected.
///
/// Closed and typed rather than a free string: the wire publishes a closed set,
/// and a stored value outside it must be a decode failure rather than a value
/// the projection has to guess at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CredentialState {
    /// Selectable.
    Ready,
    /// Revoked; sessions holding it fail closed.
    Revoked,
}

impl CredentialState {
    /// Every state, in canonical order.
    pub const ALL: [Self; 2] = [Self::Ready, Self::Revoked];

    /// The stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Revoked => "revoked",
        }
    }

    /// Resolves a stored spelling. There is no alias table.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == text)
    }
}

/// One provider-credential binding in the BYOK directory (OD-23).
///
/// The record is a **reference** to a workspace secret. A BYOK key is a
/// workspace secret, and a second copy here would be a second encryption
/// authority for the same material.
///
/// Every field the public `ProviderCredential` model needs is persisted here.
/// `fingerprint`, `name`, `revision` and `updated_at` are stored rather than
/// derived at read time: the fingerprint cannot be recomputed without the
/// plaintext, which no read path may hold, and a revision that a reader invented
/// would be useless as a concurrency token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCredential {
    /// Its identity.
    pub credential: ProviderCredentialId,
    /// The workspace.
    pub workspace: WorkspaceId,
    /// The human label the caller registered it under.
    pub name: ResourceName,
    /// Which provider it is for.
    pub provider: ProviderId,
    /// The workspace secret holding the key.
    pub secret_name: SecretName,
    /// The generation bound at registration.
    pub source_generation: SourceGeneration,
    /// The one-way fingerprint of the key material, minted at registration by
    /// the only component that ever held the plaintext.
    pub fingerprint: ContentHash,
    /// The monotone concurrency token.
    pub revision: u64,
    /// Whether the binding is usable.
    pub state: CredentialState,
    /// When it was registered.
    pub created_at: Timestamp,
    /// When it last changed.
    pub updated_at: Timestamp,
    /// When it was revoked.
    pub revoked_at: Option<Timestamp>,
}

/// Encodes one provider-credential binding.
///
/// # Errors
///
/// [`EncodeError`] when the provider could not enter a key.
pub fn encode_provider_credential(credential: &ProviderCredential) -> Result<Item, EncodeError> {
    let key = keys::provider_credential(
        credential.workspace,
        credential.provider.as_str(),
        credential.credential,
    )?;
    Ok(ItemBuilder::new(PROVIDER_CREDENTIAL)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("credentialId", s(credential.credential.to_string()))
        .set("workspaceId", s(credential.workspace.to_string()))
        .set("name", s(credential.name.as_str().to_owned()))
        .set("provider", s(credential.provider.as_str()))
        .set("secretName", s(credential.secret_name.as_str().to_owned()))
        .set("sourceGeneration", n(credential.source_generation.0))
        .set("fingerprint", s(credential.fingerprint.to_wire()))
        .set("revision", n(credential.revision))
        .set("state", s(credential.state.as_str()))
        .set("createdAt", stamp(credential.created_at))
        .set("updatedAt", stamp(credential.updated_at))
        .set_opt("revokedAt", credential.revoked_at.map(stamp))
        .build())
}

/// Decodes one provider-credential binding.
///
/// # Errors
///
/// [`CodecError`] as for every decode here, and a refusal of a record that grew
/// key material of its own.
pub fn decode_provider_credential(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<ProviderCredential, CodecError> {
    let row = Row::bind(item, PROVIDER_CREDENTIAL)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    refuse_sealed(
        item,
        PROVIDER_CREDENTIAL,
        "a credential binding references a workspace secret",
    )?;
    let provider = provider_of(row.enumerated("provider", &keys::providers())?).ok_or(
        CodecError::Malformed {
            item_type: PROVIDER_CREDENTIAL,
            attribute: "provider",
            reason: "outside the closed BYOK provider vocabulary".to_owned(),
        },
    )?;
    let fingerprint =
        ContentHash::parse(row.string("fingerprint")?).map_err(|error| CodecError::Malformed {
            item_type: PROVIDER_CREDENTIAL,
            attribute: "fingerprint",
            reason: error.to_string(),
        })?;
    Ok(ProviderCredential {
        credential: row.id::<ProviderCredentialId>("credentialId")?,
        workspace: asserted,
        name: name_of(&row, PROVIDER_CREDENTIAL)?,
        provider,
        secret_name: ResourceName::parse(row.string("secretName")?).map_err(|error| {
            CodecError::Malformed {
                item_type: PROVIDER_CREDENTIAL,
                attribute: "secretName",
                reason: error.to_string(),
            }
        })?,
        source_generation: SourceGeneration(row.u64("sourceGeneration")?),
        fingerprint,
        revision: row.u64("revision")?,
        state: CredentialState::parse(row.enumerated("state", keys::CREDENTIAL_STATES)?).ok_or(
            CodecError::Malformed {
                item_type: PROVIDER_CREDENTIAL,
                attribute: "state",
                reason: "outside the credential state vocabulary".to_owned(),
            },
        )?,
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
        revoked_at: row.opt_timestamp("revokedAt")?,
    })
}

/// Resolves a stored provider spelling against the closed generated set.
#[must_use]
pub fn provider_of(text: &str) -> Option<ProviderId> {
    ProviderId::ALL
        .iter()
        .copied()
        .find(|provider| provider.as_str() == text)
}

fn refuse_sealed(item: &Item, item_type: &'static str, reason: &str) -> Result<(), CodecError> {
    if item.contains_key("ciphertext") || item.contains_key("wrappedKey") {
        return Err(CodecError::Malformed {
            item_type,
            attribute: "ciphertext",
            reason: format!("{reason}; this row must never carry sealed bytes"),
        });
    }
    Ok(())
}

fn name_of(row: &Row<'_>, item_type: &'static str) -> Result<SecretName, CodecError> {
    ResourceName::parse(row.string("name")?).map_err(|error| CodecError::Malformed {
        item_type,
        attribute: "name",
        reason: error.to_string(),
    })
}

fn digest_of(
    row: &Row<'_>,
    item_type: &'static str,
    attribute: &'static str,
) -> Result<[u8; 32], CodecError> {
    let text = row.string(attribute)?;
    let mut bytes = [0u8; 32];
    hex::decode_to_slice(text, &mut bytes).map_err(|_| CodecError::Malformed {
        item_type,
        attribute,
        reason: "a context digest is 64 lowercase hexadecimal characters".to_owned(),
    })?;
    Ok(bytes)
}

fn uuid_of(row: &Row<'_>, attribute: &'static str) -> Result<Uuid7, CodecError> {
    let text = row.string(attribute)?;
    let mut bytes = [0u8; 16];
    hex::decode_to_slice(text, &mut bytes).map_err(|_| CodecError::Malformed {
        item_type: SESSION_CUSTODY,
        attribute,
        reason: "an owner key edge is 32 lowercase hexadecimal characters".to_owned(),
    })?;
    Uuid7::from_bytes(bytes).map_err(|error| CodecError::Malformed {
        item_type: SESSION_CUSTODY,
        attribute,
        reason: error.to_string(),
    })
}
