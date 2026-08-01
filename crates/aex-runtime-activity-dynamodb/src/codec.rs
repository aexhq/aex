//! `runtime-activity` row codecs.
//!
//! The table holds generation metadata and never customer content, which is
//! what makes its `NEW_IMAGE` stream safe. These codecs are where that stays
//! true: there is no attribute here that could carry a prompt, a body or a
//! secret, and a decode refuses anything outside the declared vocabularies.

use aex_hands_protocol::rpc::Fence;
use aex_runtime_control::generation::{GenerationState, Revision};
use aex_session_dynamodb::attr::{CodecError, Item, ItemBuilder, PK, Row, SK, n, s, stamp};
use aex_session_dynamodb::component::KeyError;
use aex_wire::ids::{GenerationId, SessionId, WorkspaceId};
use aex_wire::types::{ComputeSize, Timestamp};

use crate::keys;

/// The `itemType` of a generation head.
pub const HANDS_GENERATION: &str = "hands_generation";
/// The `itemType` of a lifecycle intent.
pub const LIFECYCLE_INTENT: &str = "lifecycle_intent";
/// The `itemType` of a lifecycle receipt.
pub const LIFECYCLE_RECEIPT: &str = "lifecycle_receipt";
/// The `itemType` of a true-idle probe.
pub const IDLE_PROBE: &str = "idle_probe";
/// The `itemType` of the current-generation pointer.
pub const CURRENT_GENERATION: &str = "current_generation";

/// Why a row could not be encoded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EncodeError {
    /// A key component was unusable.
    #[error(transparent)]
    Key(#[from] KeyError),
    /// A compute size outside the five public tokens.
    #[error("`{found}` is not one of the five public compute shapes")]
    Shape {
        /// What was offered.
        found: String,
    },
}

/// One Hands generation head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRow {
    /// The owning session.
    pub session: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The generation.
    pub generation: GenerationId,
    /// Its compute shape.
    pub size: ComputeSize,
    /// Its lifecycle state.
    pub state: GenerationState,
    /// Its lifecycle fence.
    pub fence: Fence,
    /// Its optimistic revision.
    pub revision: Revision,
    /// The provider's own identity for it, once it has one.
    pub provider_vm_id: Option<String>,
    /// The image it booted.
    pub image_identifier: Option<String>,
    /// Operations admitted and not yet settled.
    pub open_operations: u32,
    /// The last authoritatively busy instant.
    pub last_busy_at: Timestamp,
    /// When quiescence started, absent while busy.
    pub idle_since: Option<Timestamp>,
    /// When a paid keepalive lease lapses.
    pub keepalive_lease_until: Option<Timestamp>,
    /// When the provider's own lifetime ends.
    pub provider_lifetime_expires_at: Option<Timestamp>,
    /// When the reaper should look again.
    pub next_evaluate_at: Timestamp,
    /// When the head last changed.
    pub updated_at: Timestamp,
}

/// Encodes one generation head, including its due index attributes.
///
/// A terminal generation carries neither index attribute, so the reaper's scan
/// holds only generations that can still change.
///
/// # Errors
///
/// [`EncodeError`] when a key component is unusable.
pub fn encode_generation(row: &GenerationRow) -> Result<Item, EncodeError> {
    let key = keys::head(row.session, row.generation);
    let builder = ItemBuilder::new(HANDS_GENERATION)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(row.session.to_string()))
        .set("workspaceId", s(row.workspace.to_string()))
        .set("generationId", s(row.generation.to_string()))
        .set("size", s(row.size.as_str()))
        .set("state", s(keys::state_str(row.state)))
        .set("fence", n(row.fence.0))
        .set("revision", n(row.revision.value()))
        .set_opt(
            "providerVmId",
            row.provider_vm_id.as_ref().map(|id| s(id.clone())),
        )
        .set_opt(
            "imageIdentifier",
            row.image_identifier.as_ref().map(|id| s(id.clone())),
        )
        .set("openOperations", n(u64::from(row.open_operations)))
        .set("lastBusyAt", stamp(row.last_busy_at))
        .set_opt("idleSince", row.idle_since.map(stamp))
        .set_opt("keepaliveLeaseUntil", row.keepalive_lease_until.map(stamp))
        .set_opt(
            "providerLifetimeExpiresAt",
            row.provider_lifetime_expires_at.map(stamp),
        )
        .set("nextEvaluateAt", stamp(row.next_evaluate_at))
        .set("updatedAt", stamp(row.updated_at));
    let builder = if keys::is_evaluable(row.state) {
        builder
            .set(keys::DUE_PK, s(keys::due_partition(row.generation)))
            .set(
                keys::DUE_SK,
                s(keys::due_sort(row.next_evaluate_at, row.generation)),
            )
    } else {
        builder
    };
    Ok(builder.build())
}

/// Decodes one generation head and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, or a
/// row from another tenant.
pub fn decode_generation(item: &Item, asserted: WorkspaceId) -> Result<GenerationRow, CodecError> {
    let row = Row::bind(item, HANDS_GENERATION)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let size_text = row.string("size")?;
    let size = ComputeSize::ALL
        .into_iter()
        .find(|shape| shape.as_str() == size_text)
        .ok_or(CodecError::Malformed {
            item_type: HANDS_GENERATION,
            attribute: "size",
            reason: "outside the five public compute shapes".to_owned(),
        })?;
    Ok(GenerationRow {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        generation: row.id::<GenerationId>("generationId")?,
        size,
        state: keys::state_of(row.enumerated("state", keys::STATES)?).ok_or(
            CodecError::Malformed {
                item_type: HANDS_GENERATION,
                attribute: "state",
                reason: "outside the generation state vocabulary".to_owned(),
            },
        )?,
        fence: Fence(row.u64("fence")?),
        revision: Revision::new(row.u64("revision")?),
        provider_vm_id: row.opt_string("providerVmId")?.map(str::to_owned),
        image_identifier: row.opt_string("imageIdentifier")?.map(str::to_owned),
        open_operations: u32::try_from(row.u64("openOperations")?).map_err(|_| {
            CodecError::Malformed {
                item_type: HANDS_GENERATION,
                attribute: "openOperations",
                reason: "an open-operation count is a small integer".to_owned(),
            }
        })?,
        last_busy_at: row.timestamp("lastBusyAt")?,
        idle_since: row.opt_timestamp("idleSince")?,
        keepalive_lease_until: row.opt_timestamp("keepaliveLeaseUntil")?,
        provider_lifetime_expires_at: row.opt_timestamp("providerLifetimeExpiresAt")?,
        next_evaluate_at: row.timestamp("nextEvaluateAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// One lifecycle intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleIntent {
    /// The owning session.
    pub session: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The generation.
    pub generation: GenerationId,
    /// Its identity.
    pub intent_id: String,
    /// What it asks for.
    pub action: String,
    /// The fence it was taken under.
    pub requested_fence: Fence,
    /// Where it is.
    pub state: String,
    /// The provider's request identity, once dispatched.
    pub provider_request_id: Option<String>,
    /// When it was requested.
    pub requested_at: Timestamp,
    /// When it was dispatched.
    pub dispatched_at: Option<Timestamp>,
}

/// Encodes one lifecycle intent.
///
/// # Errors
///
/// [`EncodeError`] when the intent identity could not enter a key.
pub fn encode_intent(intent: &LifecycleIntent) -> Result<Item, EncodeError> {
    let key = keys::intent(intent.session, intent.generation, &intent.intent_id)?;
    Ok(ItemBuilder::new(LIFECYCLE_INTENT)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(intent.session.to_string()))
        .set("workspaceId", s(intent.workspace.to_string()))
        .set("generationId", s(intent.generation.to_string()))
        .set("intentId", s(intent.intent_id.clone()))
        .set("action", s(intent.action.clone()))
        .set("requestedFence", n(intent.requested_fence.0))
        .set("state", s(intent.state.clone()))
        .set_opt(
            "providerRequestId",
            intent.provider_request_id.as_ref().map(|id| s(id.clone())),
        )
        .set("requestedAt", stamp(intent.requested_at))
        .set_opt("dispatchedAt", intent.dispatched_at.map(stamp))
        .build())
}

/// Decodes one lifecycle intent.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_intent(item: &Item, asserted: WorkspaceId) -> Result<LifecycleIntent, CodecError> {
    let row = Row::bind(item, LIFECYCLE_INTENT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(LifecycleIntent {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        generation: row.id::<GenerationId>("generationId")?,
        intent_id: row.string("intentId")?.to_owned(),
        action: row.enumerated("action", keys::ACTIONS)?.to_owned(),
        requested_fence: Fence(row.u64("requestedFence")?),
        state: row.enumerated("state", keys::INTENT_STATES)?.to_owned(),
        provider_request_id: row.opt_string("providerRequestId")?.map(str::to_owned),
        requested_at: row.timestamp("requestedAt")?,
        dispatched_at: row.opt_timestamp("dispatchedAt")?,
    })
}

/// One immutable lifecycle receipt.
///
/// Receipts carry **no TTL**: they are the lifecycle evidence usage facts
/// reference, and reclaiming one on a timer would remove the basis of a bill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleReceipt {
    /// The owning session.
    pub session: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The generation.
    pub generation: GenerationId,
    /// Which intent it settles.
    pub intent_id: String,
    /// How it settled.
    pub outcome: String,
    /// The state the provider reported.
    pub observed_state: String,
    /// The provider's error code, when it failed.
    pub provider_error_code: Option<String>,
    /// Active milliseconds, for the compute meter.
    pub active_ms: Option<u64>,
    /// Suspended milliseconds.
    pub suspended_ms: Option<u64>,
    /// Retained snapshot byte-milliseconds, for the storage meter.
    pub snapshot_retained_byte_ms: Option<u64>,
    /// When it settled.
    pub settled_at: Timestamp,
}

/// Encodes one lifecycle receipt.
///
/// # Errors
///
/// [`EncodeError`] when the intent identity could not enter a key.
pub fn encode_receipt(receipt: &LifecycleReceipt) -> Result<Item, EncodeError> {
    let key = keys::receipt(receipt.session, receipt.generation, &receipt.intent_id)?;
    Ok(ItemBuilder::new(LIFECYCLE_RECEIPT)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(receipt.session.to_string()))
        .set("workspaceId", s(receipt.workspace.to_string()))
        .set("generationId", s(receipt.generation.to_string()))
        .set("intentId", s(receipt.intent_id.clone()))
        .set("outcome", s(receipt.outcome.clone()))
        .set("observedState", s(receipt.observed_state.clone()))
        .set_opt(
            "providerErrorCode",
            receipt
                .provider_error_code
                .as_ref()
                .map(|code| s(code.clone())),
        )
        .set_opt("activeMs", receipt.active_ms.map(n))
        .set_opt("suspendedMs", receipt.suspended_ms.map(n))
        .set_opt(
            "snapshotRetainedByteMs",
            receipt.snapshot_retained_byte_ms.map(n),
        )
        .set("settledAt", stamp(receipt.settled_at))
        .build())
}

/// Decodes one lifecycle receipt.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_receipt(item: &Item, asserted: WorkspaceId) -> Result<LifecycleReceipt, CodecError> {
    let row = Row::bind(item, LIFECYCLE_RECEIPT)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(LifecycleReceipt {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        generation: row.id::<GenerationId>("generationId")?,
        intent_id: row.string("intentId")?.to_owned(),
        outcome: row.enumerated("outcome", keys::OUTCOMES)?.to_owned(),
        observed_state: row.enumerated("observedState", keys::STATES)?.to_owned(),
        provider_error_code: row.opt_string("providerErrorCode")?.map(str::to_owned),
        active_ms: row.opt_u64("activeMs")?,
        suspended_ms: row.opt_u64("suspendedMs")?,
        snapshot_retained_byte_ms: row.opt_u64("snapshotRetainedByteMs")?,
        settled_at: row.timestamp("settledAt")?,
    })
}

/// One true-idle probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleProbe {
    /// The owning session.
    pub session: SessionId,
    /// The owning workspace.
    pub workspace: WorkspaceId,
    /// The generation.
    pub generation: GenerationId,
    /// When it was observed.
    pub observed_at: Timestamp,
    /// Operations open at that instant.
    pub open_operations: u32,
    /// Operations queued at that instant.
    pub queued_operations: u32,
    /// Operations admitted at that instant.
    pub admitted_operations: u32,
    /// A keepalive lease in force, when there was one.
    pub keepalive_lease_until: Option<Timestamp>,
}

/// Encodes one true-idle probe, with the only TTL on this table.
#[must_use]
pub fn encode_probe(probe: &IdleProbe) -> Item {
    let key = keys::probe(probe.session, probe.generation, probe.observed_at);
    ItemBuilder::new(IDLE_PROBE)
        .set(PK, s(key.pk))
        .set(SK, s(key.sk))
        .set("sessionId", s(probe.session.to_string()))
        .set("workspaceId", s(probe.workspace.to_string()))
        .set("generationId", s(probe.generation.to_string()))
        .set("observedAt", stamp(probe.observed_at))
        .set("openOperations", n(u64::from(probe.open_operations)))
        .set("queuedOperations", n(u64::from(probe.queued_operations)))
        .set(
            "admittedOperations",
            n(u64::from(probe.admitted_operations)),
        )
        .set_opt(
            "keepaliveLeaseUntil",
            probe.keepalive_lease_until.map(stamp),
        )
        .set(
            "expiresAtEpochSeconds",
            crate::expressions::probe_ttl(probe.observed_at),
        )
        .build()
}

/// The pointer a Brain activation reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentGeneration {
    /// The session.
    pub session: SessionId,
    /// The generation it currently points at.
    pub generation: GenerationId,
    /// The fence it was pointed under.
    pub fence: Fence,
    /// The optimistic revision.
    pub revision: Revision,
    /// When it last changed.
    pub updated_at: Timestamp,
}

/// Decodes the current-generation pointer.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_current(item: &Item) -> Result<CurrentGeneration, CodecError> {
    let row = Row::bind(item, CURRENT_GENERATION)?;
    Ok(CurrentGeneration {
        session: row.id::<SessionId>("sessionId")?,
        generation: row.id::<GenerationId>("generationId")?,
        fence: Fence(row.u64("fence")?),
        revision: Revision::new(row.u64("revision")?),
        updated_at: row.timestamp("updatedAt")?,
    })
}
