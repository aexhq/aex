//! The one place a regional authority value becomes a wire model, and a wire
//! request becomes a domain command.
//!
//! # Why it lives here
//!
//! The projection cannot live in the domain or the application layer: a domain
//! that returned `aex_wire::models::*` would depend on the wire, which inverts
//! the dependency the whole rewrite is built on. It cannot live in one
//! deployable either, because `regional:secrets` and
//! `regional:provider-credentials` are each split across two deployables — a
//! copy in each is two spellings of one representation, and they would drift.
//! `aex-regional-http` already owns the wire boundary for every regional
//! deployable, so it is where the two vocabularies are allowed to meet.
//!
//! # What "total" means here
//!
//! Every function is total over its input or returns a typed
//! [`ProjectionError`]. There is no `unwrap`, no `unwrap_or_default`, and no
//! field filled in to satisfy a type:
//!
//! * a value the authority does not hold is **not invented** — the projection
//!   refuses and the route answers a declared error;
//! * a stored state outside the published set is a refusal, never a guess;
//! * an entity tag is derived from the projected representation itself, so
//!   "the tag changed" and "the body changed" cannot disagree.

use aex_content_domain::identity::RegistryKind;
use aex_secret_custody_dynamodb::codec::{
    CredentialState, ProviderCredential as StoredCredential, SecretMetadata as StoredSecret,
};
use aex_secret_domain::plaintext::{PlaintextError, SecretPlaintext};
use aex_secret_domain::secret::{SecretName, SecretState};
use aex_session_dynamodb::paging::PagePosition;
use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::models;
use aex_wire::types::{DecimalU128, ETag};
use aex_workspace_domain::registry::RegistryPointer;
use serde::Serialize;

use crate::cursor::{CursorError, SortTuple};

/// Why a value could not be projected onto the wire, or a request onto a domain
/// command.
///
/// Each variant is a fact about the value, not a formatting accident, and each
/// maps onto exactly one published error code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
    /// The record is a tombstone. A deleted secret has no public
    /// representation: it is absent, not a `deleted` state the wire can carry.
    #[error("secret `{name}` is deleted and has no public representation")]
    SecretDeleted {
        /// Which name.
        name: SecretName,
    },
    /// A registry pointer was projected onto the model of another registry.
    ///
    /// A pointer is keyed by `(workspace, kind, name)`, so the kind is the only
    /// thing that distinguishes two identically named rows. Projecting one into
    /// the wrong model would publish a skill as a tool.
    #[error("expected a `{expected}` registry pointer, found a `{found}` one")]
    WrongRegistryKind {
        /// Which registry the route reads.
        expected: &'static str,
        /// Which registry the row belongs to.
        found: &'static str,
    },
    /// A revocation receipt was asked for from a record that carries no
    /// revocation instant.
    #[error("secret `{name}` carries no revocation instant")]
    NotRevoked {
        /// Which name.
        name: SecretName,
    },
    /// The request body could not become a domain value.
    #[error(transparent)]
    Plaintext(#[from] PlaintextError),
    /// A continuation could not be issued or consumed.
    #[error(transparent)]
    Cursor(#[from] CursorError),
    /// A projected model could not be canonicalized, which is a defect in this
    /// module rather than a customer condition.
    #[error("a projected representation is not canonicalizable")]
    NotCanonicalizable,
}

impl ProjectionError {
    /// The published code this refusal answers with.
    ///
    /// Exhaustive by construction: a new variant fails to compile here rather
    /// than degrading to `internal_error` at runtime.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            Self::SecretDeleted { .. } => ErrorCode::NotFound,
            // A revocation receipt over an unrevoked record, a
            // non-canonicalizable model and a pointer read out of the wrong
            // registry are all invariant failures of this process. The last one
            // in particular means the key template and the query disagreed,
            // which a caller can neither cause nor fix.
            Self::NotRevoked { .. } | Self::NotCanonicalizable | Self::WrongRegistryKind { .. } => {
                ErrorCode::InternalError
            }
            Self::Plaintext(_) => ErrorCode::InvalidRequest,
            Self::Cursor(_) => ErrorCode::InvalidCursor,
        }
    }
}

impl From<ProjectionError> for WireError {
    fn from(error: ProjectionError) -> Self {
        let code = error.code();
        // The message is the typed refusal's own rendering; it never carries a
        // stored value, a plaintext or an upstream provider string.
        Self::new(code).with_message(error.to_string())
    }
}

// --- authority failures ----------------------------------------------------------

/// Renders one authority failure as a published code.
///
/// Exhaustive over the store vocabulary, so a new failure mode is a compile
/// error here rather than an unclassified `500` in production. Three groups:
///
/// * a lost condition is the caller's `precondition_failed`;
/// * contention, throttling and an unavailable service are transient and are
///   reported as such;
/// * everything else — a corrupt row, an unusable key, an oversized item, an
///   absent table, a denied role — is this system's fault and is never dressed
///   up as a customer condition.
///
/// A route that declares none of the transient codes turns the transient answer
/// into `internal_error` at the dispatch boundary. That is the route table's
/// gap, and it is visible in the message rather than hidden.
#[must_use]
pub fn authority_failure(error: &aex_session_dynamodb::error::StoreError) -> WireError {
    use aex_session_dynamodb::error::StoreError;

    let code = match error {
        StoreError::PreconditionFailed { .. } => ErrorCode::PreconditionFailed,
        StoreError::Contended | StoreError::Throttled { .. } => ErrorCode::RateLimited,
        StoreError::Unavailable { .. } | StoreError::CommitAmbiguous { .. } => {
            ErrorCode::UpstreamError
        }
        StoreError::IdempotencyConflict => ErrorCode::IdempotencyConflict,
        StoreError::Invalid { .. }
        | StoreError::Misconfigured { .. }
        | StoreError::Denied
        | StoreError::Corrupt(_)
        | StoreError::Key(_)
        | StoreError::ItemTooLarge { .. } => ErrorCode::InternalError,
    };
    WireError::new(code)
}

// --- secrets -------------------------------------------------------------------

/// Whether a stored record is still admissible.
///
/// This is not the stored `state` column. An emergency revoke is one conditional
/// update that raises `revokedThroughRevision` to the current revision and
/// leaves the state alone (that is what makes it `O(1)` in the generation
/// count), and every use path admits only while
/// `revokedThroughRevision < revision`. The public `state` therefore has to be
/// read from the same comparison the admission path uses, or the customer would
/// be told `ready` about a record nothing may use.
#[must_use]
fn public_state(stored: &StoredSecret) -> Option<models::SecretState> {
    match stored.state {
        SecretState::Deleted => None,
        SecretState::Revoked => Some(models::SecretState::Revoked),
        SecretState::Ready => {
            if stored.revoked_through_revision < stored.revision {
                Some(models::SecretState::Ready)
            } else {
                Some(models::SecretState::Revoked)
            }
        }
    }
}

/// Projects one stored secret record onto its public metadata.
///
/// The value is never readable, and there is nowhere in the target type to put
/// one: [`StoredSecret`] cannot hold a ciphertext and neither can
/// [`models::SecretMetadata`].
///
/// # Errors
///
/// [`ProjectionError::SecretDeleted`] for a tombstone, which is `404` rather
/// than a `deleted` state on the wire.
pub fn secret_metadata(stored: &StoredSecret) -> Result<models::SecretMetadata, ProjectionError> {
    let state = public_state(stored).ok_or_else(|| ProjectionError::SecretDeleted {
        name: stored.name.clone(),
    })?;
    Ok(models::SecretMetadata {
        created_at: stored.created_at,
        name: stored.name.clone(),
        revision: stored.revision.0,
        revoked_at: stored.revoked_at,
        state,
        updated_at: stored.updated_at,
    })
}

/// Projects one page of stored secret records.
///
/// A tombstone is **skipped**, not refused: a listing is a view of what exists,
/// and one deleted row must not make the whole page unanswerable.
///
/// # Errors
///
/// [`ProjectionError`] only from the continuation.
pub fn secret_metadata_page(
    stored: &[StoredSecret],
    next_cursor: Option<Cursor>,
) -> Result<models::SecretMetadataPage, ProjectionError> {
    Ok(models::SecretMetadataPage {
        items: stored
            .iter()
            .filter_map(|row| secret_metadata(row).ok())
            .collect(),
        next_cursor,
    })
}

/// Projects the receipt of a committed revocation.
///
/// # Errors
///
/// [`ProjectionError::NotRevoked`] when the record carries no revocation
/// instant, which can only happen if a revocation was reported without being
/// committed.
pub fn secret_revocation(
    stored: &StoredSecret,
) -> Result<models::SecretRevocation, ProjectionError> {
    let revoked_at = stored
        .revoked_at
        .ok_or_else(|| ProjectionError::NotRevoked {
            name: stored.name.clone(),
        })?;
    Ok(models::SecretRevocation {
        name: stored.name.clone(),
        revision: stored.revision.0,
        revoked_at,
    })
}

/// Projects the write-only `PUT` body onto the domain plaintext.
///
/// The bytes leave the request body and enter [`SecretPlaintext`] in one move,
/// so the only owner of the value from here on is a type that cannot be
/// serialized and zeroizes on drop.
///
/// # Errors
///
/// [`ProjectionError::Plaintext`] for an empty or over-long value.
pub fn secret_plaintext(
    request: models::SecretPutRequest,
) -> Result<SecretPlaintext, ProjectionError> {
    Ok(SecretPlaintext::new(request.value.into_bytes())?)
}

// --- provider credentials --------------------------------------------------------

/// Projects one stored BYOK binding.
///
/// Total with no refusal: every field the model requires is a persisted column,
/// which is exactly why they were added to the row rather than derived here.
/// The `fingerprint` in particular cannot be recomputed on this path — the
/// plaintext is not reachable from a metadata read — so a projection that
/// synthesised one would be publishing a value with no relationship to the key.
#[must_use]
pub fn provider_credential(stored: &StoredCredential) -> models::ProviderCredential {
    models::ProviderCredential {
        created_at: stored.created_at,
        fingerprint: stored.fingerprint,
        id: stored.credential,
        name: stored.name.clone(),
        provider: stored.provider,
        revision: stored.revision,
        revoked_at: stored.revoked_at,
        state: match stored.state {
            CredentialState::Ready => models::ProviderCredentialState::Ready,
            CredentialState::Revoked => models::ProviderCredentialState::Revoked,
        },
        updated_at: stored.updated_at,
    }
}

/// Projects one page of stored BYOK bindings.
#[must_use]
pub fn provider_credential_page(
    stored: &[StoredCredential],
    next_cursor: Option<Cursor>,
) -> models::ProviderCredentialPage {
    models::ProviderCredentialPage {
        items: stored.iter().map(provider_credential).collect(),
        next_cursor,
    }
}

// --- registry ---------------------------------------------------------------------

/// Projects one registry pointer onto its **collection row**.
///
/// The five registry models carry an identical field set and differ only in the
/// type of the `value` they may carry, so this produces the shared part once and
/// the five wrappers below place it. Writing five copies of the same seven
/// assignments is how two of them eventually disagree.
///
/// # Why the row never carries a value
///
/// A registry pointer stores a `sha256` and a size; the value itself lives in
/// content storage as a **sealed** body. Reading it needs the content data key,
/// which the deployable that serves these listings deliberately does not hold.
/// The wire model marks `value` "omitted in collection rows", which is exactly
/// what a listing publishes — so a listing is complete, and an item read that
/// must carry the value is not servable from this projection alone.
///
/// # Errors
///
/// [`ProjectionError::WrongRegistryKind`] when the pointer belongs to another
/// registry. A pointer is keyed by `(workspace, kind, name)` and the kind is the
/// only thing distinguishing two identically named rows, so projecting one into
/// the wrong model would publish a skill as a tool.
fn registry_row(
    pointer: &RegistryPointer,
    expected: RegistryKind,
) -> Result<RegistryRow, ProjectionError> {
    if pointer.kind != expected {
        return Err(ProjectionError::WrongRegistryKind {
            expected: kind_name(expected),
            found: kind_name(pointer.kind),
        });
    }
    Ok(RegistryRow {
        created_at: pointer.created_at,
        name: pointer.name.clone(),
        revision: pointer.revision.0,
        sha256: pointer.sha256,
        size_bytes: DecimalU128::new(u128::from(pointer.size_bytes)),
        state: models::RegisteredState::Current,
        updated_at: pointer.updated_at,
    })
}

/// The field set every registry collection row shares.
struct RegistryRow {
    created_at: aex_wire::types::Timestamp,
    name: aex_wire::ids::ResourceName,
    revision: u64,
    sha256: aex_wire::ids::ContentHash,
    size_bytes: DecimalU128,
    state: models::RegisteredState,
    updated_at: aex_wire::types::Timestamp,
}

/// The registry's own spelling of a kind, for a diagnostic.
const fn kind_name(kind: RegistryKind) -> &'static str {
    match kind {
        RegistryKind::File => "file",
        RegistryKind::Skill => "skill",
        RegistryKind::Tool => "tool",
        RegistryKind::Instruction => "instruction",
        RegistryKind::McpServer => "mcp_server",
    }
}

/// Emits the five per-kind projections and their page forms.
///
/// A macro rather than five hand-written pairs: the five wire types are distinct
/// structs with identical fields, so the only thing that varies is the type
/// name, and hand-writing the variation is hand-writing the drift.
macro_rules! registry_projections {
    ($($item:ident, $page:ident, $kind:ident, $row:ident, $rows:ident;)+) => {
        $(
            #[doc = concat!("Projects one stored `", stringify!($kind), "` pointer onto its collection row.")]
            ///
            /// # Errors
            ///
            /// [`ProjectionError::WrongRegistryKind`] for a pointer from another
            /// registry.
            pub fn $row(pointer: &RegistryPointer) -> Result<models::$item, ProjectionError> {
                let row = registry_row(pointer, RegistryKind::$kind)?;
                Ok(models::$item {
                    created_at: row.created_at,
                    name: row.name,
                    revision: row.revision,
                    sha256: row.sha256,
                    size_bytes: row.size_bytes,
                    state: row.state,
                    updated_at: row.updated_at,
                    // Omitted in a collection row, and this projection produces
                    // only collection rows.
                    value: None,
                })
            }

            #[doc = concat!("Projects one page of stored `", stringify!($kind), "` pointers.")]
            ///
            /// # Errors
            ///
            /// As the row projection. A page fails whole rather than skipping a
            /// row: unlike a tombstone, a pointer from the wrong registry is a
            /// corrupt read, not an absent resource.
            pub fn $rows(
                pointers: &[RegistryPointer],
                next_cursor: Option<Cursor>,
            ) -> Result<models::$page, ProjectionError> {
                Ok(models::$page {
                    items: pointers.iter().map($row).collect::<Result<Vec<_>, _>>()?,
                    next_cursor,
                })
            }
        )+
    };
}

registry_projections! {
    RegisteredFile, RegisteredFilePage, File, registered_file, registered_file_page;
    RegisteredSkill, RegisteredSkillPage, Skill, registered_skill, registered_skill_page;
    RegisteredTool, RegisteredToolPage, Tool, registered_tool, registered_tool_page;
    RegisteredInstruction, RegisteredInstructionPage, Instruction,
        registered_instruction, registered_instruction_page;
    RegisteredMcpServer, RegisteredMcpServerPage, McpServer,
        registered_mcp_server, registered_mcp_server_page;
}

// --- continuations ---------------------------------------------------------------

/// Renders an authority page position as the cursor's ordered sort tuple.
///
/// The regional cursor carries an ordered tuple of authority fields. A base
/// table position is its two key attributes; a secondary-index position is the
/// base pair followed by the complete index pair. Nothing else is carried, so
/// a cursor can never hold a body or a receipt even if the row it points at
/// does.
///
/// # Errors
///
/// [`ProjectionError::Cursor`] when a key attribute is outside the tuple bound.
pub fn position_tuple(position: &PagePosition) -> Result<SortTuple, ProjectionError> {
    let parts = match (&position.index_pk, &position.index_sk) {
        (None, None) => vec![position.pk.clone(), position.sk.clone()],
        (Some(secondary_partition), Some(secondary_sort)) => vec![
            position.pk.clone(),
            position.sk.clone(),
            secondary_partition.clone(),
            secondary_sort.clone(),
        ],
        _ => return Err(ProjectionError::Cursor(CursorError::Malformed)),
    };
    Ok(SortTuple::new(parts)?)
}

/// Reads an authority page position back out of a verified sort tuple.
///
/// # Errors
///
/// [`ProjectionError::Cursor`] when the tuple is neither a two-part base key nor
/// a four-part index position. The tuple is authenticated before it reaches
/// here, so a wrong arity is a cursor minted for another collection rather than
/// a parse accident — the same refusal either way.
pub fn tuple_position(tuple: &SortTuple) -> Result<PagePosition, ProjectionError> {
    match tuple.parts() {
        [pk, sk] => Ok(PagePosition {
            pk: pk.clone(),
            sk: sk.clone(),
            index_pk: None,
            index_sk: None,
        }),
        [pk, sk, secondary_partition, secondary_sort] => Ok(PagePosition {
            pk: pk.clone(),
            sk: sk.clone(),
            index_pk: Some(secondary_partition.clone()),
            index_sk: Some(secondary_sort.clone()),
        }),
        _ => Err(ProjectionError::Cursor(CursorError::Malformed)),
    }
}

// --- entity tags -----------------------------------------------------------------

/// The strong entity tag of one projected representation.
///
/// Derived from the **representation itself**, canonicalized with the one
/// workspace canonicalizer (RFC 8785 JCS) and domain-separated by the model
/// name, rather than from a revision column. Two properties follow, and both are
/// asserted:
///
/// * the tag changes whenever any published field changes, including a change
///   that does not advance a revision — an emergency revoke is exactly that
///   case, since it raises the revocation fence and leaves the revision alone;
/// * two resources of different kinds that happen to carry equal field values
///   never share a tag.
///
/// The value is quoted, which is what makes it a *strong* validator in
/// `If-Match` and `ETag`.
///
/// # Errors
///
/// [`ProjectionError::NotCanonicalizable`] if the model cannot be canonicalized,
/// which would be a defect in the generated model rather than a data condition.
pub fn entity_tag<T: Serialize>(kind: &str, value: &T) -> Result<ETag, ProjectionError> {
    use sha2::Digest as _;

    let body = aex_wire::canonical::to_jcs_bytes(value)
        .map_err(|_| ProjectionError::NotCanonicalizable)?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"aex.regional.etag.v1");
    let kind = kind.as_bytes();
    hasher.update(u32::try_from(kind.len()).unwrap_or(u32::MAX).to_be_bytes());
    hasher.update(kind);
    hasher.update(&body);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut rendered = String::with_capacity(66);
    rendered.push('"');
    for byte in digest {
        rendered.push(char::from(hex_digit(byte >> 4)));
        rendered.push(char::from(hex_digit(byte & 0x0f)));
    }
    rendered.push('"');
    ETag::parse(&rendered).map_err(|_| ProjectionError::NotCanonicalizable)
}

const fn hex_digit(nibble: u8) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble,
        _ => b'a' + (nibble - 10),
    }
}
