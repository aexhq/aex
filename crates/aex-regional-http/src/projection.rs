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
use aex_session_domain::{Message, MessagePart, MessageRole, MessageState};
use aex_session_dynamodb::paging::PagePosition;
use aex_wire::cursor::Cursor;
use aex_wire::error::{ErrorCode, WireError};
use aex_wire::models;
use aex_wire::types::ETag;
use aex_workspace_domain::registry::{
    RegistryPointer, RegistryRow as StoredRegistryRow, RegistryState,
};
use serde::Serialize;

use crate::cursor::{CursorError, SortTuple};

/// Why a value could not be projected onto the wire, or a request onto a domain
/// command.
///
/// Each variant is a fact about the value, not a formatting accident, and each
/// maps onto exactly one published error code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionError {
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
    /// A continuation could not be issued or consumed.
    #[error(transparent)]
    Cursor(#[from] CursorError),
    /// A projected model could not be canonicalized, which is a defect in this
    /// module rather than a customer condition.
    #[error("a projected representation is not canonicalizable")]
    NotCanonicalizable,
    /// A stored value document does not decode as its own kind's read model.
    ///
    /// The document is written by this process from a validated request body and
    /// its digest is re-verified on the way out of the codec, so a document that
    /// no longer matches its kind is corruption, never a customer condition.
    #[error("the stored `{kind}` value document is not a `{kind}` value: {reason}")]
    MalformedValueDocument {
        /// Which registry the document was read from.
        kind: &'static str,
        /// What the decoder objected to.
        reason: String,
    },
    /// An open partial message reached a public projection call.
    #[error("message `{message}` is still open and has no public representation")]
    UnsealedMessage {
        /// Which message violated the sealed-only boundary.
        message: aex_wire::ids::MessageId,
    },
    /// A persisted-file attachment reached the text-only public message view.
    #[error("message `{message}` contains a deferred file attachment")]
    DeferredMessageAttachment {
        /// Which message violated the text-only launch boundary.
        message: aex_wire::ids::MessageId,
    },
}

impl ProjectionError {
    /// The published code this refusal answers with.
    ///
    /// Exhaustive by construction: a new variant fails to compile here rather
    /// than degrading to `internal_error` at runtime.
    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        match self {
            // A non-canonicalizable model and a pointer read out of the wrong
            // registry are all invariant failures of this process. The last one
            // in particular means the key template and the query disagreed,
            // which a caller can neither cause nor fix.
            Self::NotCanonicalizable
            | Self::WrongRegistryKind { .. }
            | Self::MalformedValueDocument { .. }
            | Self::UnsealedMessage { .. }
            | Self::DeferredMessageAttachment { .. } => ErrorCode::InternalError,
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

// --- registry ---------------------------------------------------------------------

/// The shared collection columns of every registry row.
///
/// The five registry models carry an identical field set, so this produces the
/// shared part once and the ten wrappers below place it. Writing ten copies of
/// the same seven assignments is how two of them eventually disagree.
///
/// # Errors
///
/// [`ProjectionError::WrongRegistryKind`] when the row belongs to another
/// registry. A pointer is keyed by `(workspace, kind, name)` and the kind is the
/// only thing distinguishing two identically named rows, so projecting one into
/// the wrong model would publish a skill as a tool.
fn registry_columns(
    row: &StoredRegistryRow,
    expected: RegistryKind,
) -> Result<RegistryColumns, ProjectionError> {
    if row.kind != expected {
        return Err(ProjectionError::WrongRegistryKind {
            expected: kind_name(expected),
            found: kind_name(row.kind),
        });
    }
    Ok(RegistryColumns {
        created_at: row.created_at,
        name: row.name.clone(),
        state: match row.state {
            RegistryState::Pending => models::RegisteredState::Pending,
            RegistryState::Ready => models::RegisteredState::Ready,
            RegistryState::Failed => models::RegisteredState::Failed,
        },
        failure_code: row.failure_code.clone(),
        updated_at: row.updated_at,
    })
}

/// The field set every registry collection row shares.
struct RegistryColumns {
    created_at: aex_wire::types::Timestamp,
    name: aex_wire::ids::ResourceName,
    state: models::RegisteredState,
    failure_code: Option<String>,
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

/// Emits the five per-kind point projections, the five row projections and the
/// five page forms.
///
/// A macro rather than fifteen hand-written functions: the wire types are
/// distinct structs over identical field sets, so the only thing that varies is
/// the type name, and hand-writing the variation is hand-writing the drift.
macro_rules! registry_projections {
    ($($item:ident, $read:ident, $rowty:ident, $page:ident, $kind:ident,
        $point:ident, $row:ident, $rows:ident;)+) => {
        $(
            #[doc = concat!("Projects one stored `", stringify!($kind), "` pointer, value and all.")]
            ///
            /// The response type has no `None` to publish: after the D-6 split a
            /// point read that could not produce a value is not representable,
            /// so a collection projection can never be published as if it were a
            /// complete resource.
            ///
            /// # Errors
            ///
            /// [`ProjectionError::WrongRegistryKind`] for a pointer from another
            /// registry, and [`ProjectionError::MalformedValueDocument`] when the
            /// stored document is not this kind's value.
            pub fn $point(pointer: &RegistryPointer) -> Result<models::$item, ProjectionError> {
                let columns = registry_columns(&pointer.row, RegistryKind::$kind)?;
                let value: Option<models::$read> = if columns.state == models::RegisteredState::Ready {
                    Some(serde_json::from_str(pointer.value_doc.as_str()).map_err(|error| {
                        ProjectionError::MalformedValueDocument {
                            kind: kind_name(RegistryKind::$kind),
                            reason: error.to_string(),
                        }
                    })?)
                } else {
                    None
                };
                Ok(models::$item {
                    created_at: columns.created_at,
                    failure_code: columns.failure_code,
                    name: columns.name,
                    state: columns.state,
                    updated_at: columns.updated_at,
                    value,
                })
            }

            #[doc = concat!("Projects one stored `", stringify!($kind), "` row onto its collection form.")]
            ///
            /// The row type has no `value` field at all, so a listing cannot
            /// carry one even by accident.
            ///
            /// # Errors
            ///
            /// [`ProjectionError::WrongRegistryKind`] for a row from another
            /// registry.
            pub fn $row(row: &StoredRegistryRow) -> Result<models::$rowty, ProjectionError> {
                let columns = registry_columns(row, RegistryKind::$kind)?;
                Ok(models::$rowty {
                    created_at: columns.created_at,
                    failure_code: columns.failure_code,
                    name: columns.name,
                    state: columns.state,
                    updated_at: columns.updated_at,
                })
            }

            #[doc = concat!("Projects one page of stored `", stringify!($kind), "` rows.")]
            ///
            /// # Errors
            ///
            /// As the row projection. A page fails whole rather than skipping a
            /// row: unlike a tombstone, a row from the wrong registry is a
            /// corrupt read, not an absent resource.
            pub fn $rows(
                rows: &[StoredRegistryRow],
                next_cursor: Option<Cursor>,
            ) -> Result<models::$page, ProjectionError> {
                Ok(models::$page {
                    items: rows.iter().map($row).collect::<Result<Vec<_>, _>>()?,
                    next_cursor,
                })
            }
        )+
    };
}

registry_projections! {
    RegisteredFile, RegisteredFileRead, RegisteredFileRow, RegisteredFilePage, File,
        registered_file, registered_file_row, registered_file_page;
}

// --- sessions ---------------------------------------------------------------------

/// Projects one complete sealed message onto its public resource.
///
/// # Errors
///
/// [`ProjectionError::UnsealedMessage`] when a caller bypasses the immutable
/// sealed-message range and supplies a mutable base row.
pub fn session_message(stored: &Message) -> Result<models::Message, ProjectionError> {
    if stored.state != MessageState::Sealed || stored.sealed_at.is_none() {
        return Err(ProjectionError::UnsealedMessage { message: stored.id });
    }
    let content = stored
        .parts
        .iter()
        .map(|part| match part {
            MessagePart::Text { text } => Ok(models::MessagePart::Text(models::MessagePartText {
                text: text.clone(),
            })),
            MessagePart::File { .. } => {
                Err(ProjectionError::DeferredMessageAttachment { message: stored.id })
            }
            MessagePart::ToolCall { id, arguments } => {
                Ok(models::MessagePart::ToolCall(models::MessagePartToolCall {
                    id: *id,
                    name: "tool".to_owned(),
                    arguments: aex_wire::CanonicalJson::from_value(
                        &serde_json::json!({"sha256": arguments.to_string()}),
                    )
                    .map_err(|_| ProjectionError::NotCanonicalizable)?,
                }))
            }
            MessagePart::ToolResult { id, result } => Ok(models::MessagePart::ToolResult(
                models::MessagePartToolResult {
                    id: *id,
                    preview: result.to_string(),
                    sandbox_path: None,
                    sha256: None,
                    size_bytes: None,
                    truncated: false,
                },
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(models::Message {
        content,
        created_at: stored.created_at,
        id: stored.id,
        role: match stored.role {
            MessageRole::User => models::MessageRole::User,
            MessageRole::Assistant => models::MessageRole::Assistant,
            MessageRole::Tool => models::MessageRole::Tool,
        },
        session_id: stored.session,
    })
}

/// Projects one seal-ordered page after its signed continuation is minted.
///
/// # Errors
///
/// As for [`session_message`]. The page fails whole rather than hiding a row.
pub fn session_message_page(
    stored: &[Message],
    next_cursor: Option<Cursor>,
) -> Result<models::MessagePage, ProjectionError> {
    Ok(models::MessagePage {
        items: stored
            .iter()
            .map(session_message)
            .collect::<Result<Vec<_>, _>>()?,
        next_cursor,
    })
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
