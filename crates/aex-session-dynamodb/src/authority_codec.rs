//! Versioned, lossless codecs for the canonical session-domain authority.
//!
//! One canonical JSON document owns every domain field, while a small checked
//! set of top-level attributes exists solely for conditions and the workspace
//! list index. A decode never supplies a missing default.

use std::num::NonZeroU64;

use aex_content_domain::ContentDigest;
use aex_internal_contracts::RunId;
use aex_operation_domain::{DeletionEpoch, DeletionGuard, DeletionState, OperationKind};
use aex_secret_domain::CustodyRevision;
use aex_session_domain::{
    CancellationEpoch, DomainError, EffectId, InterruptReason, Lineage, Message, MessagePart,
    MessageRole, MessageState, MutationGuard, Origin, ResolvedConfigAuthority,
    ResolvedConfigDigest, Run, RunOutcome, Session, SessionRevision, SessionStatus, WorkAdmission,
};
use aex_wire::CanonicalJson;
use aex_wire::ids::{
    AgentId, FilePath, GenerationId, MessageId, OperationId, OrganizationId, SessionId,
    TelemetryGapId, ToolCallId, Uuid7, WorkspaceId,
};
use aex_wire::provider::ProviderId;
use aex_wire::types::Timestamp;
use serde::{Deserialize, Serialize};

use crate::attr::{CodecError, Item, ItemBuilder, Row, n, s};
use crate::{codec, keys};

/// The first strict-v1 authority document format.
pub const AUTHORITY_SCHEMA_VERSION: u64 = 1;
/// The stored document version attribute.
pub const AUTHORITY_SCHEMA: &str = "authoritySchemaVersion";
/// The canonical authority document attribute.
pub const AUTHORITY_DOCUMENT: &str = "authorityDocument";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MutationGuardV1 {
    holder: OperationId,
    kind: String,
    acquired_at: Timestamp,
}

impl From<MutationGuard> for MutationGuardV1 {
    fn from(guard: MutationGuard) -> Self {
        Self {
            holder: guard.holder,
            kind: guard.kind.as_str().to_owned(),
            acquired_at: guard.acquired_at,
        }
    }
}

impl MutationGuardV1 {
    fn decode(self) -> Result<MutationGuard, CodecError> {
        Ok(MutationGuard {
            holder: self.holder,
            kind: OperationKind::parse(&self.kind)
                .ok_or_else(|| malformed(AUTHORITY_DOCUMENT, "unknown mutation operation kind"))?,
            acquired_at: self.acquired_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DeletionV1 {
    state: String,
    epoch: u64,
    trashed_at: Option<Timestamp>,
    recovery_deadline: Option<Timestamp>,
    purge_operation: Option<OperationId>,
}

impl From<DeletionGuard> for DeletionV1 {
    fn from(guard: DeletionGuard) -> Self {
        Self {
            state: deletion_state(guard.state).to_owned(),
            epoch: guard.epoch.0,
            trashed_at: guard.trashed_at,
            recovery_deadline: guard.recovery_deadline,
            purge_operation: guard.purge_operation,
        }
    }
}

impl DeletionV1 {
    fn decode(self, session: SessionId) -> Result<DeletionGuard, CodecError> {
        Ok(DeletionGuard {
            session,
            state: parse_deletion_state(&self.state)?,
            epoch: DeletionEpoch(self.epoch),
            trashed_at: self.trashed_at,
            recovery_deadline: self.recovery_deadline,
            purge_operation: self.purge_operation,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct OriginV1 {
    session: SessionId,
    operation: OperationId,
    cloned_at: Timestamp,
}

impl From<Origin> for OriginV1 {
    fn from(origin: Origin) -> Self {
        Self {
            session: origin.session,
            operation: origin.operation,
            cloned_at: origin.cloned_at,
        }
    }
}

impl OriginV1 {
    fn decode(self) -> Result<Origin, CodecError> {
        Ok(Origin {
            session: self.session,
            operation: self.operation,
            cloned_at: self.cloned_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ResolvedV1 {
    digest: String,
    document: String,
    provider: ProviderId,
    model: String,
}

impl From<&ResolvedConfigAuthority> for ResolvedV1 {
    fn from(config: &ResolvedConfigAuthority) -> Self {
        Self {
            digest: hex::encode(config.digest().0),
            document: config.document().as_str().to_owned(),
            provider: config.provider(),
            model: config.model().to_owned(),
        }
    }
}

impl ResolvedV1 {
    fn decode(self) -> Result<ResolvedConfigAuthority, CodecError> {
        let document = CanonicalJson::parse(&self.document)
            .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))?;
        let config = ResolvedConfigAuthority::new(document, self.provider, self.model)
            .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))?;
        let recorded = ResolvedConfigDigest(decode_digest(&self.digest, AUTHORITY_DOCUMENT)?);
        if config.digest() != recorded {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "resolved configuration digest does not match its document",
            ));
        }
        Ok(config)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct SessionV1 {
    id: SessionId,
    workspace: WorkspaceId,
    organization: OrganizationId,
    status: String,
    revision: u64,
    active_run: Option<RunId>,
    work_admission: String,
    cancellation: u64,
    deletion: DeletionV1,
    mutation_guard: Option<MutationGuardV1>,
    root_agent: AgentId,
    generation: Option<GenerationId>,
    /// The immutable generation definition the create decided (A D-2).
    ///
    /// The whole `HandsGeneration` tuple rather than a projection of it: the
    /// first launch derives the two `runtime-activity` rows from this as a
    /// total function, and a lossy projection would force that derivation to
    /// invent whatever it could not read back.
    pinned_runtime: aex_runtime_control::generation::HandsGeneration,
    custody_revision: u64,
    origin: Option<OriginV1>,
    resolved: ResolvedV1,
    metadata_document: Option<String>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl From<&Session> for SessionV1 {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id,
            workspace: session.workspace,
            organization: session.organization,
            status: session_status(session.status).to_owned(),
            revision: session.revision.0,
            active_run: session.active_run,
            work_admission: work_admission(session.work_admission).to_owned(),
            cancellation: session.cancellation.0,
            deletion: session.deletion.into(),
            mutation_guard: session.mutation_guard.map(Into::into),
            root_agent: session.root_agent,
            generation: session.generation,
            pinned_runtime: session.pinned_runtime.definition().clone(),
            custody_revision: session.custody_revision.0,
            origin: session.lineage.origin.map(Into::into),
            resolved: (&session.resolved).into(),
            metadata_document: session
                .metadata
                .as_ref()
                .map(|metadata| metadata.document().as_str().to_owned()),
            created_at: session.created_at,
            updated_at: session.updated_at,
        }
    }
}

impl SessionV1 {
    fn decode(self) -> Result<Session, CodecError> {
        let id = self.id;
        Ok(Session {
            id,
            workspace: self.workspace,
            organization: self.organization,
            status: parse_session_status(&self.status)?,
            revision: SessionRevision(self.revision),
            active_run: self.active_run,
            work_admission: parse_work_admission(&self.work_admission)?,
            cancellation: CancellationEpoch(self.cancellation),
            deletion: self.deletion.decode(id)?,
            mutation_guard: self
                .mutation_guard
                .map(MutationGuardV1::decode)
                .transpose()?,
            root_agent: self.root_agent,
            generation: self.generation,
            // Re-checked on the way in, not trusted: the pin's identity triple
            // is re-asserted against the head's own, so a row whose pinned
            // definition names another tenant is a decode failure rather than a
            // generation another session could launch.
            pinned_runtime: aex_session_domain::PinnedRuntime::new(
                id,
                self.workspace,
                self.organization,
                self.pinned_runtime,
            )
            .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))?,
            custody_revision: CustodyRevision(self.custody_revision),
            lineage: Lineage {
                origin: self.origin.map(OriginV1::decode).transpose()?,
            },
            resolved: self.resolved.decode()?,
            metadata: self
                .metadata_document
                .map(|document| {
                    let document = CanonicalJson::parse(&document)
                        .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))?;
                    aex_session_domain::SessionMetadata::new(document)
                        .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))
                })
                .transpose()?,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "snake_case")]
enum MessagePartV1 {
    Text {
        text: String,
    },
    File {
        path: FilePath,
        media_type: Option<String>,
        source: String,
    },
    ToolCall {
        id: ToolCallId,
        arguments: ContentDigest,
    },
    ToolResult {
        id: ToolCallId,
        result: ContentDigest,
    },
}

impl From<&MessagePart> for MessagePartV1 {
    fn from(part: &MessagePart) -> Self {
        match part {
            MessagePart::Text { text } => Self::Text { text: text.clone() },
            MessagePart::File { path, media_type } => Self::File {
                path: path.clone(),
                media_type: media_type.clone(),
                source: "persisted".to_owned(),
            },
            MessagePart::ToolCall { id, arguments } => Self::ToolCall {
                id: *id,
                arguments: *arguments,
            },
            MessagePart::ToolResult { id, result } => Self::ToolResult {
                id: *id,
                result: *result,
            },
        }
    }
}

impl MessagePartV1 {
    fn decode(self) -> Result<MessagePart, CodecError> {
        Ok(match self {
            Self::Text { text } => MessagePart::Text { text },
            Self::File {
                path,
                media_type,
                source,
            } => {
                if source != "persisted" {
                    return Err(malformed(AUTHORITY_DOCUMENT, "unknown file source"));
                }
                MessagePart::File { path, media_type }
            }
            Self::ToolCall { id, arguments } => MessagePart::ToolCall { id, arguments },
            Self::ToolResult { id, result } => MessagePart::ToolResult { id, result },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct MessageV1 {
    id: MessageId,
    session: SessionId,
    run: Option<RunId>,
    agent: AgentId,
    role: String,
    state: String,
    parts: Vec<MessagePartV1>,
    created_at: Timestamp,
    sealed_at: Option<Timestamp>,
}

impl From<&Message> for MessageV1 {
    fn from(message: &Message) -> Self {
        Self {
            id: message.id,
            session: message.session,
            run: message.run,
            agent: message.agent,
            role: message_role(message.role).to_owned(),
            state: message_state(message.state).to_owned(),
            parts: message.parts.iter().map(Into::into).collect(),
            created_at: message.created_at,
            sealed_at: message.sealed_at,
        }
    }
}

impl MessageV1 {
    fn decode(self) -> Result<Message, CodecError> {
        if (self.state == "open") != self.sealed_at.is_none() {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "message state and seal instant disagree",
            ));
        }
        Ok(Message {
            id: self.id,
            session: self.session,
            run: self.run,
            agent: self.agent,
            role: parse_message_role(&self.role)?,
            state: parse_message_state(&self.state)?,
            parts: self
                .parts
                .into_iter()
                .map(MessagePartV1::decode)
                .collect::<Result<Vec<_>, _>>()?,
            created_at: self.created_at,
            sealed_at: self.sealed_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "snake_case")]
enum InterruptV1 {
    StopRequested { operation: OperationId },
    AccountPaused,
    AuthorizationRevoked,
    ContinuityLost { generation: Option<GenerationId> },
    SpendCapExhausted,
    AmbiguousEffect { effect: Uuid7 },
}

impl From<InterruptReason> for InterruptV1 {
    fn from(reason: InterruptReason) -> Self {
        match reason {
            InterruptReason::StopRequested { operation } => Self::StopRequested { operation },
            InterruptReason::AccountPaused => Self::AccountPaused,
            InterruptReason::AuthorizationRevoked => Self::AuthorizationRevoked,
            InterruptReason::ContinuityLost { generation } => Self::ContinuityLost { generation },
            InterruptReason::SpendCapExhausted => Self::SpendCapExhausted,
            InterruptReason::AmbiguousEffect { effect } => {
                Self::AmbiguousEffect { effect: effect.0 }
            }
        }
    }
}

impl From<InterruptV1> for InterruptReason {
    fn from(reason: InterruptV1) -> Self {
        match reason {
            InterruptV1::StopRequested { operation } => Self::StopRequested { operation },
            InterruptV1::AccountPaused => Self::AccountPaused,
            InterruptV1::AuthorizationRevoked => Self::AuthorizationRevoked,
            InterruptV1::ContinuityLost { generation } => Self::ContinuityLost { generation },
            InterruptV1::SpendCapExhausted => Self::SpendCapExhausted,
            InterruptV1::AmbiguousEffect { effect } => Self::AmbiguousEffect {
                effect: EffectId(effect),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "snake_case")]
enum RunOutcomeV1 {
    Succeeded {
        output_messages: Vec<MessageId>,
    },
    Failed {
        code: aex_wire::error::ErrorCode,
        message: String,
        detail: Option<CanonicalJson>,
        retryable: bool,
    },
    TimedOut {
        deadline: Timestamp,
    },
    Cancelled {
        by: OperationId,
    },
    Interrupted {
        reason: InterruptV1,
    },
}

impl From<&RunOutcome> for RunOutcomeV1 {
    fn from(outcome: &RunOutcome) -> Self {
        match outcome {
            RunOutcome::Succeeded { output_messages } => Self::Succeeded {
                output_messages: output_messages.clone(),
            },
            RunOutcome::Failed { error } => Self::Failed {
                code: error.code,
                message: error.message.clone(),
                detail: error.detail.clone(),
                retryable: error.retryable,
            },
            RunOutcome::TimedOut { deadline } => Self::TimedOut {
                deadline: *deadline,
            },
            RunOutcome::Cancelled { by } => Self::Cancelled { by: *by },
            RunOutcome::Interrupted(reason) => Self::Interrupted {
                reason: (*reason).into(),
            },
        }
    }
}

impl From<RunOutcomeV1> for RunOutcome {
    fn from(outcome: RunOutcomeV1) -> Self {
        match outcome {
            RunOutcomeV1::Succeeded { output_messages } => Self::Succeeded { output_messages },
            RunOutcomeV1::Failed {
                code,
                message,
                detail,
                retryable,
            } => Self::Failed {
                error: DomainError {
                    code,
                    message,
                    detail,
                    retryable,
                },
            },
            RunOutcomeV1::TimedOut { deadline } => Self::TimedOut { deadline },
            RunOutcomeV1::Cancelled { by } => Self::Cancelled { by },
            RunOutcomeV1::Interrupted { reason } => Self::Interrupted(reason.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RunV1 {
    id: RunId,
    session: SessionId,
    message: MessageId,
    status: aex_session_domain::RunStatus,
    max_spend_cents: u64,
    deadline: Timestamp,
    cancellation_at_admission: u64,
    queued_at: Timestamp,
    started_at: Option<Timestamp>,
    terminal_at: Option<Timestamp>,
    outcome: Option<RunOutcomeV1>,
    telemetry_complete: Option<bool>,
    telemetry_gaps: Option<Vec<TelemetryGapId>>,
}

impl From<&Run> for RunV1 {
    fn from(run: &Run) -> Self {
        Self {
            id: run.id,
            session: run.session,
            message: run.message,
            status: run.status,
            max_spend_cents: run.max_spend_cents.get(),
            deadline: run.deadline,
            cancellation_at_admission: run.cancellation_at_admission.0,
            queued_at: run.queued_at,
            started_at: run.started_at,
            terminal_at: run.terminal_at,
            outcome: run.outcome.as_ref().map(Into::into),
            telemetry_complete: run.telemetry_complete,
            telemetry_gaps: run.telemetry_gaps.clone(),
        }
    }
}

impl RunV1 {
    fn decode(self) -> Result<Run, CodecError> {
        let max_spend_cents = NonZeroU64::new(self.max_spend_cents)
            .ok_or_else(|| malformed(AUTHORITY_DOCUMENT, "run spend ceiling is zero"))?;
        let outcome = self.outcome.map(Into::into);
        if self.status.is_terminal() != outcome.is_some() {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "run status and terminal outcome disagree",
            ));
        }
        if outcome.as_ref().map(RunOutcome::status) != Some(self.status)
            && self.status.is_terminal()
        {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "run outcome and terminal status disagree",
            ));
        }
        if self.status.is_terminal() != self.terminal_at.is_some() {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "run status and terminal instant disagree",
            ));
        }
        if self.status == aex_session_domain::RunStatus::Queued && self.started_at.is_some() {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "queued run has a start instant",
            ));
        }
        if self.status == aex_session_domain::RunStatus::Running && self.started_at.is_none() {
            return Err(malformed(
                AUTHORITY_DOCUMENT,
                "running run has no start instant",
            ));
        }
        Ok(Run {
            id: self.id,
            session: self.session,
            message: self.message,
            status: self.status,
            max_spend_cents,
            deadline: self.deadline,
            cancellation_at_admission: CancellationEpoch(self.cancellation_at_admission),
            queued_at: self.queued_at,
            started_at: self.started_at,
            terminal_at: self.terminal_at,
            outcome,
            telemetry_complete: self.telemetry_complete,
            telemetry_gaps: self.telemetry_gaps,
        })
    }
}

/// Encodes a canonical session head and its complete no-hydration list document.
///
/// # Errors
///
/// Returns [`CodecError`] only if canonical serialization fails.
pub fn encode_session(session: &Session) -> Result<Item, CodecError> {
    let document = encode_document(&SessionV1::from(session))?;
    let key = keys::head(session.id);
    let lifecycle = deletion_state(session.deletion.state);
    let indexed = session.deletion.state != DeletionState::Purged;
    let builder = ItemBuilder::new(codec::SESSION_HEAD)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set(AUTHORITY_SCHEMA, n(AUTHORITY_SCHEMA_VERSION))
        .set(AUTHORITY_DOCUMENT, s(document))
        .set("sessionId", s(session.id.to_string()))
        .set("workspaceId", s(session.workspace.to_string()))
        .set("organizationId", s(session.organization.to_string()))
        .set("status", s(session_status(session.status)))
        .set("lifecycle", s(lifecycle))
        .set("workAdmission", s(work_admission(session.work_admission)))
        .set("revision", n(session.revision.0))
        .set("deletionEpoch", n(session.deletion.epoch.0))
        .set("cancelEpoch", n(session.cancellation.0))
        .set_opt(
            "activeRunId",
            session.active_run.map(|run| s(run.to_string())),
        )
        .set("rootAgentId", s(session.root_agent.to_string()))
        .set_opt(
            "mutationGuardOperationId",
            session
                .mutation_guard
                .map(|guard| s(guard.holder.to_string())),
        )
        .set(
            "resolvedConfigDigest",
            s(aex_wire::ids::ContentHash::from_bytes(session.resolved.digest().0).to_wire()),
        )
        .set("provider", s(session.resolved.provider().as_str()))
        .set("model", s(session.resolved.model().to_owned()))
        .set("custodyRevision", n(session.custody_revision.0))
        .set("createdAt", crate::attr::stamp(session.created_at))
        .set("updatedAt", crate::attr::stamp(session.updated_at));
    let builder = if indexed {
        builder
            .set(
                keys::workspace_index::PK,
                s(keys::workspace_index::session_partition(session.workspace)),
            )
            .set(
                keys::workspace_index::SK,
                s(keys::workspace_index::session_sort(
                    session.created_at,
                    session.id,
                )),
            )
    } else {
        builder
    };
    Ok(builder.build())
}

/// Decodes a canonical session head read from the base table.
///
/// # Errors
///
/// Returns [`CodecError`] for every missing, malformed, cross-tenant or
/// internally inconsistent authority field.
pub fn decode_session(item: &Item, asserted: WorkspaceId) -> Result<Session, CodecError> {
    let row = Row::bind(item, codec::SESSION_HEAD)?;
    decode_session_row(row, asserted)
}

/// Decodes a complete authority document after a workspace-index locator was
/// strongly hydrated from the base table.
///
/// The workspace index is `KEYS_ONLY`; it never acts as session truth.
///
/// # Errors
///
/// Returns [`CodecError`] for every missing, malformed, cross-tenant or
/// internally inconsistent authority field.
pub fn decode_session_projection(
    item: &Item,
    asserted: WorkspaceId,
) -> Result<Session, CodecError> {
    decode_session_row(Row::bind(item, codec::SESSION_HEAD)?, asserted)
}

fn decode_session_row(row: Row<'_>, asserted: WorkspaceId) -> Result<Session, CodecError> {
    row.owned_by("workspaceId", &asserted.to_string())?;
    require_schema(&row)?;
    let expected_id = row.id::<SessionId>("sessionId")?;
    let session: SessionV1 = decode_document(row.string(AUTHORITY_DOCUMENT)?)?;
    if session.id != expected_id || session.workspace != asserted {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "session document disagrees with its indexed identity",
        ));
    }
    let session = session.decode()?;
    expect_key(&row, &keys::head(session.id))?;
    if row.string("status")? != session_status(session.status)
        || row.string("lifecycle")? != deletion_state(session.deletion.state)
        || row.string("workAdmission")? != work_admission(session.work_admission)
        || row.u64("revision")? != session.revision.0
        || row.u64("deletionEpoch")? != session.deletion.epoch.0
        || row.u64("cancelEpoch")? != session.cancellation.0
        || row.opt_run_id("activeRunId")? != session.active_run
        || row.id::<AgentId>("rootAgentId")? != session.root_agent
        || row.opt_id::<OperationId>("mutationGuardOperationId")?
            != session.mutation_guard.map(|guard| guard.holder)
        || row.id::<OrganizationId>("organizationId")? != session.organization
        || row.string("provider")? != session.resolved.provider().as_str()
        || row.string("model")? != session.resolved.model()
        || row.u64("custodyRevision")? != session.custody_revision.0
        || row.timestamp("createdAt")? != session.created_at
        || row.timestamp("updatedAt")? != session.updated_at
    {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "session document disagrees with its checked list projections",
        ));
    }
    let expected_config_digest =
        aex_wire::ids::ContentHash::from_bytes(session.resolved.digest().0).to_wire();
    if row.string("resolvedConfigDigest")? != expected_config_digest {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "session document disagrees with its resolved configuration digest projection",
        ));
    }
    let expected_index_partition = keys::workspace_index::session_partition(asserted);
    let expected_index_sort = keys::workspace_index::session_sort(session.created_at, session.id);
    let indexed = session.deletion.state != DeletionState::Purged;
    if row.opt_string(keys::workspace_index::PK)?
        != indexed.then_some(expected_index_partition.as_str())
        || row.opt_string(keys::workspace_index::SK)?
            != indexed.then_some(expected_index_sort.as_str())
    {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "session document disagrees with its sparse index placement",
        ));
    }
    Ok(session)
}

/// Encodes one canonical domain message.
///
/// # Errors
///
/// Returns [`CodecError`] when the canonical document cannot be encoded.
pub fn encode_domain_message(
    message: &Message,
    workspace: WorkspaceId,
    organization: OrganizationId,
) -> Result<Item, CodecError> {
    let key = keys::message(message.session, message.id);
    Ok(ItemBuilder::new(codec::MESSAGE)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set(AUTHORITY_SCHEMA, n(AUTHORITY_SCHEMA_VERSION))
        .set(
            AUTHORITY_DOCUMENT,
            s(encode_document(&MessageV1::from(message))?),
        )
        .set("messageId", s(message.id.to_string()))
        .set("sessionId", s(message.session.to_string()))
        .set("workspaceId", s(workspace.to_string()))
        .set("organizationId", s(organization.to_string()))
        .set("agentId", s(message.agent.to_string()))
        .set("state", s(message_state(message.state)))
        .set_opt("runId", message.run.map(|run| s(run.to_string())))
        .set("createdAt", crate::attr::stamp(message.created_at))
        .build())
}

/// Decodes one canonical domain message.
///
/// # Errors
///
/// Returns [`CodecError`] for every missing, malformed, cross-tenant or
/// internally inconsistent authority field.
pub fn decode_domain_message(item: &Item, asserted: WorkspaceId) -> Result<Message, CodecError> {
    let row = Row::bind(item, codec::MESSAGE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    require_schema(&row)?;
    let expected_id = row.id::<MessageId>("messageId")?;
    let expected_session = row.id::<SessionId>("sessionId")?;
    let stored: MessageV1 = decode_document(row.string(AUTHORITY_DOCUMENT)?)?;
    if stored.id != expected_id || stored.session != expected_session {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "message document disagrees with its indexed identity",
        ));
    }
    let message = stored.decode()?;
    expect_key(&row, &keys::message(message.session, message.id))?;
    let _organization = row.id::<OrganizationId>("organizationId")?;
    if row.id::<AgentId>("agentId")? != message.agent
        || row.string("state")? != message_state(message.state)
        || row.opt_run_id("runId")? != message.run
        || row.timestamp("createdAt")? != message.created_at
    {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "message document disagrees with its checked projection",
        ));
    }
    Ok(message)
}

/// Encodes the immutable public projection of one sealed message.
///
/// # Errors
///
/// Refuses an open message or a sealed message without its seal instant. The
/// row is complete rather than a locator so listing is one strong range read.
pub fn encode_sealed_message(
    message: &Message,
    workspace: WorkspaceId,
    organization: OrganizationId,
) -> Result<Item, CodecError> {
    if message.state != MessageState::Sealed {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "an open message cannot enter the sealed-message projection",
        ));
    }
    let sealed_at = message.sealed_at.ok_or_else(|| {
        malformed(
            AUTHORITY_DOCUMENT,
            "a sealed-message projection requires a seal instant",
        )
    })?;
    let key = keys::sealed_message(message.session, sealed_at, message.id);
    Ok(ItemBuilder::new(codec::SEALED_MESSAGE)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set(AUTHORITY_SCHEMA, n(AUTHORITY_SCHEMA_VERSION))
        .set(
            AUTHORITY_DOCUMENT,
            s(encode_document(&MessageV1::from(message))?),
        )
        .set("messageId", s(message.id.to_string()))
        .set("sessionId", s(message.session.to_string()))
        .set("workspaceId", s(workspace.to_string()))
        .set("organizationId", s(organization.to_string()))
        .set("agentId", s(message.agent.to_string()))
        .set_opt("runId", message.run.map(|run| s(run.to_string())))
        .set("createdAt", crate::attr::stamp(message.created_at))
        .set("sealedAt", crate::attr::stamp(sealed_at))
        .build())
}

/// Decodes and verifies one immutable sealed-message projection.
///
/// # Errors
///
/// Returns [`CodecError`] for an incomplete, cross-tenant, open, or
/// key/document-inconsistent row.
pub fn decode_sealed_message(item: &Item, asserted: WorkspaceId) -> Result<Message, CodecError> {
    let row = Row::bind(item, codec::SEALED_MESSAGE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    require_schema(&row)?;
    let expected_id = row.id::<MessageId>("messageId")?;
    let expected_session = row.id::<SessionId>("sessionId")?;
    let stored: MessageV1 = decode_document(row.string(AUTHORITY_DOCUMENT)?)?;
    if stored.id != expected_id || stored.session != expected_session {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "sealed-message document disagrees with its indexed identity",
        ));
    }
    let message = stored.decode()?;
    if message.state != MessageState::Sealed {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "a sealed-message projection contains an open message",
        ));
    }
    let sealed_at = message.sealed_at.ok_or_else(|| {
        malformed(
            AUTHORITY_DOCUMENT,
            "a sealed-message projection has no seal instant",
        )
    })?;
    expect_key(
        &row,
        &keys::sealed_message(message.session, sealed_at, message.id),
    )?;
    let _organization = row.id::<OrganizationId>("organizationId")?;
    if row.id::<AgentId>("agentId")? != message.agent
        || row.opt_run_id("runId")? != message.run
        || row.timestamp("createdAt")? != message.created_at
        || row.timestamp("sealedAt")? != sealed_at
    {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "sealed-message document disagrees with its checked projection",
        ));
    }
    Ok(message)
}

/// Encodes one canonical domain run.
///
/// # Errors
///
/// Returns [`CodecError`] when the canonical document cannot be encoded.
pub fn encode_domain_run(
    run: &Run,
    workspace: WorkspaceId,
    organization: OrganizationId,
) -> Result<Item, CodecError> {
    let key = keys::run(run.session, run.id);
    Ok(ItemBuilder::new(codec::RUN)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set(AUTHORITY_SCHEMA, n(AUTHORITY_SCHEMA_VERSION))
        .set(AUTHORITY_DOCUMENT, s(encode_document(&RunV1::from(run))?))
        .set("runId", s(run.id.to_string()))
        .set("sessionId", s(run.session.to_string()))
        .set("workspaceId", s(workspace.to_string()))
        .set("organizationId", s(organization.to_string()))
        .set("status", s(run_status(run.status)))
        .set("queuedAt", crate::attr::stamp(run.queued_at))
        .build())
}

/// Decodes one canonical domain run.
///
/// # Errors
///
/// Returns [`CodecError`] for every missing, malformed, cross-tenant or
/// internally inconsistent authority field.
pub fn decode_domain_run(item: &Item, asserted: WorkspaceId) -> Result<Run, CodecError> {
    let row = Row::bind(item, codec::RUN)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    require_schema(&row)?;
    let expected_id = row.run_id("runId")?;
    let expected_session = row.id::<SessionId>("sessionId")?;
    let stored: RunV1 = decode_document(row.string(AUTHORITY_DOCUMENT)?)?;
    if stored.id != expected_id || stored.session != expected_session {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "run document disagrees with its indexed identity",
        ));
    }
    let run = stored.decode()?;
    expect_key(&row, &keys::run(run.session, run.id))?;
    let _organization = row.id::<OrganizationId>("organizationId")?;
    if row.string("status")? != run_status(run.status)
        || row.timestamp("queuedAt")? != run.queued_at
    {
        return Err(malformed(
            AUTHORITY_DOCUMENT,
            "run document disagrees with its checked projections",
        ));
    }
    Ok(run)
}

fn encode_document(value: &impl Serialize) -> Result<String, CodecError> {
    aex_wire::canonical::to_jcs_string(value)
        .map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))
}

fn decode_document<T: for<'de> Deserialize<'de>>(text: &str) -> Result<T, CodecError> {
    serde_json::from_str(text).map_err(|error| malformed(AUTHORITY_DOCUMENT, error.to_string()))
}

fn require_schema(row: &Row<'_>) -> Result<(), CodecError> {
    let version = row.u64(AUTHORITY_SCHEMA)?;
    if version != AUTHORITY_SCHEMA_VERSION {
        return Err(malformed(AUTHORITY_SCHEMA, "unsupported authority schema"));
    }
    Ok(())
}

fn expect_key(row: &Row<'_>, expected: &keys::Key) -> Result<(), CodecError> {
    if row.string(crate::attr::PK)? == expected.pk && row.string(crate::attr::SK)? == expected.sk {
        return Ok(());
    }
    Err(malformed(
        AUTHORITY_DOCUMENT,
        "authority document disagrees with its exact physical key",
    ))
}

fn decode_digest(text: &str, attribute: &'static str) -> Result<[u8; 32], CodecError> {
    let bytes = hex::decode(text).map_err(|error| malformed(attribute, error.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| malformed(attribute, "digest is not 32 bytes"))
}

fn malformed(attribute: &'static str, reason: impl Into<String>) -> CodecError {
    CodecError::Malformed {
        item_type: "canonical_authority",
        attribute,
        reason: reason.into(),
    }
}

const fn session_status(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "idle",
        SessionStatus::Running => "running",
        SessionStatus::AwaitingApproval => "awaiting_approval",
        SessionStatus::Trashed => "trashed",
        SessionStatus::Purging => "purging",
    }
}

fn parse_session_status(text: &str) -> Result<SessionStatus, CodecError> {
    match text {
        "idle" => Ok(SessionStatus::Idle),
        "running" => Ok(SessionStatus::Running),
        "awaiting_approval" => Ok(SessionStatus::AwaitingApproval),
        "trashed" => Ok(SessionStatus::Trashed),
        "purging" => Ok(SessionStatus::Purging),
        _ => Err(malformed(AUTHORITY_DOCUMENT, "unknown session status")),
    }
}

const fn work_admission(admission: WorkAdmission) -> &'static str {
    match admission {
        WorkAdmission::Open => "open",
        WorkAdmission::Paused => "paused",
        WorkAdmission::Trashing => "trashing",
        WorkAdmission::Purging => "purging",
        WorkAdmission::ContinuityLost => "continuity_lost",
    }
}

fn parse_work_admission(text: &str) -> Result<WorkAdmission, CodecError> {
    match text {
        "open" => Ok(WorkAdmission::Open),
        "paused" => Ok(WorkAdmission::Paused),
        "trashing" => Ok(WorkAdmission::Trashing),
        "purging" => Ok(WorkAdmission::Purging),
        "continuity_lost" => Ok(WorkAdmission::ContinuityLost),
        _ => Err(malformed(AUTHORITY_DOCUMENT, "unknown work admission")),
    }
}

const fn deletion_state(state: DeletionState) -> &'static str {
    match state {
        DeletionState::Live => "active",
        DeletionState::Trashed => "trashed",
        DeletionState::Purging => "purging",
        DeletionState::Purged => "purged",
    }
}

fn parse_deletion_state(text: &str) -> Result<DeletionState, CodecError> {
    match text {
        "active" => Ok(DeletionState::Live),
        "trashed" => Ok(DeletionState::Trashed),
        "purging" => Ok(DeletionState::Purging),
        "purged" => Ok(DeletionState::Purged),
        _ => Err(malformed(AUTHORITY_DOCUMENT, "unknown deletion state")),
    }
}

const fn message_role(role: MessageRole) -> &'static str {
    match role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn parse_message_role(text: &str) -> Result<MessageRole, CodecError> {
    match text {
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        "tool" => Ok(MessageRole::Tool),
        _ => Err(malformed(AUTHORITY_DOCUMENT, "unknown message role")),
    }
}

const fn message_state(state: MessageState) -> &'static str {
    match state {
        MessageState::Open => "open",
        MessageState::Sealed => "sealed",
    }
}

fn parse_message_state(text: &str) -> Result<MessageState, CodecError> {
    match text {
        "open" => Ok(MessageState::Open),
        "sealed" => Ok(MessageState::Sealed),
        _ => Err(malformed(AUTHORITY_DOCUMENT, "unknown message state")),
    }
}

/// The one stable spelling of a run status, shared by the run row and the outbox
/// row so a relay and a reader cannot disagree about a terminal state.
pub(crate) const fn run_status(status: aex_session_domain::RunStatus) -> &'static str {
    use aex_session_domain::RunStatus;
    match status {
        RunStatus::Queued => "queued",
        RunStatus::Running => "running",
        RunStatus::Succeeded => "succeeded",
        RunStatus::Failed => "failed",
        RunStatus::TimedOut => "timed_out",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Interrupted => "interrupted",
    }
}

#[cfg(test)]
mod tests {
    use aex_content_domain::ContentDigest;
    use aex_session_domain::testing::{id, moment, running_session, session_fixture};
    use aex_session_domain::{MessagePart, RunOutcome, seal};
    use aex_wire::ids::{FilePath, OperationId, TelemetryGapId, ToolCallId};
    use aws_sdk_dynamodb::types::AttributeValue;

    use super::{
        AUTHORITY_DOCUMENT, AUTHORITY_SCHEMA, decode_domain_message, decode_domain_run,
        decode_sealed_message, decode_session, decode_session_projection, encode_domain_message,
        encode_domain_run, encode_sealed_message, encode_session,
    };

    #[test]
    fn session_round_trip_preserves_every_authority_field_and_projection() {
        let mut session = session_fixture();
        session.metadata = Some(
            aex_session_domain::SessionMetadata::new(
                aex_wire::CanonicalJson::parse(r#"{"answer":9007199254740993,"flag":true}"#)
                    .expect("canonical metadata"),
            )
            .expect("scalar metadata"),
        );
        session.updated_at = moment(9);
        let item = encode_session(&session).expect("encode");
        assert!(item.contains_key(AUTHORITY_SCHEMA));
        assert!(item.contains_key(AUTHORITY_DOCUMENT));
        assert_eq!(
            decode_session(&item, session.workspace),
            Ok(session.clone())
        );
        let projection: crate::attr::Item = item
            .into_iter()
            .filter(|(name, _)| {
                matches!(
                    name.as_str(),
                    "pk" | "sk"
                        | "wsIndexPk"
                        | "wsIndexSk"
                        | "itemType"
                        | "sessionId"
                        | "workspaceId"
                        | "status"
                        | "lifecycle"
                        | "workAdmission"
                        | "createdAt"
                        | "updatedAt"
                        | "revision"
                        | "deletionEpoch"
                        | "cancelEpoch"
                        | "activeRunId"
                        | "rootAgentId"
                        | "mutationGuardOperationId"
                        | "organizationId"
                        | "provider"
                        | "model"
                        | "custodyRevision"
                        | "resolvedConfigDigest"
                        | AUTHORITY_SCHEMA
                        | AUTHORITY_DOCUMENT
                )
            })
            .collect();
        assert_eq!(
            decode_session_projection(&projection, session.workspace),
            Ok(session)
        );
    }

    #[test]
    fn message_round_trip_keeps_file_and_tool_parts() {
        let (session, _run, _agent, mut message) = running_session();
        message.parts = vec![
            MessagePart::File {
                path: FilePath::parse("/report.json").expect("path"),
                media_type: Some("application/json".to_owned()),
            },
            MessagePart::ToolCall {
                id: id::<ToolCallId>(31),
                arguments: ContentDigest::of(b"arguments"),
            },
            MessagePart::ToolResult {
                id: id::<ToolCallId>(31),
                result: ContentDigest::of(b"result"),
            },
        ];
        let item = encode_domain_message(&message, session.workspace, session.organization)
            .expect("encode");
        assert_eq!(decode_domain_message(&item, session.workspace), Ok(message));
    }

    #[test]
    fn only_a_complete_sealed_message_enters_the_public_projection() {
        let (session, _run, _agent, open) = running_session();
        assert!(
            encode_sealed_message(&open, session.workspace, session.organization).is_err(),
            "an open partial message must remain invisible"
        );

        let sealed = seal(&open, moment(10)).message;
        let item = encode_sealed_message(&sealed, session.workspace, session.organization)
            .expect("sealed projection");
        let expected_sort = format!("SEALEDMSG#{}#{}", moment(10).to_wire(), sealed.id);
        assert_eq!(
            item.get(crate::attr::SK)
                .and_then(|value| value.as_s().ok()),
            Some(&expected_sort)
        );
        assert_eq!(decode_sealed_message(&item, session.workspace), Ok(sealed));
    }

    #[test]
    fn run_round_trip_keeps_typed_terminal_reason_and_telemetry_state() {
        let (session, mut run, _agent, _message) = running_session();
        run.status = aex_session_domain::RunStatus::Cancelled;
        run.terminal_at = Some(moment(10));
        run.outcome = Some(RunOutcome::Cancelled {
            by: id::<OperationId>(32),
        });
        run.telemetry_complete = Some(false);
        run.telemetry_gaps = Some(vec![id::<TelemetryGapId>(33)]);
        let item =
            encode_domain_run(&run, session.workspace, session.organization).expect("encode");
        assert_eq!(decode_domain_run(&item, session.workspace), Ok(run));
    }

    #[test]
    fn unknown_schema_and_cross_tenant_rows_fail_closed() {
        let session = session_fixture();
        let mut item = encode_session(&session).expect("encode");
        item.insert(AUTHORITY_SCHEMA.to_owned(), crate::attr::n(2));
        assert!(decode_session(&item, session.workspace).is_err());

        let item = encode_session(&session).expect("encode");
        assert!(decode_session(&item, id::<aex_wire::ids::WorkspaceId>(99)).is_err());
    }

    #[test]
    fn every_authority_row_binds_its_exact_physical_key() {
        let (session, run, _agent, message) = running_session();
        let mut session_item = encode_session(&session).expect("session");
        session_item.insert("sk".to_owned(), crate::attr::s("RUN#wrong"));
        assert!(decode_session(&session_item, session.workspace).is_err());

        let mut message_item =
            encode_domain_message(&message, session.workspace, session.organization)
                .expect("message");
        message_item.insert("pk".to_owned(), crate::attr::s("SESSION#wrong"));
        assert!(decode_domain_message(&message_item, session.workspace).is_err());

        let mut run_item =
            encode_domain_run(&run, session.workspace, session.organization).expect("run");
        run_item.insert("sk".to_owned(), crate::attr::s("RUN#wrong"));
        assert!(decode_domain_run(&run_item, session.workspace).is_err());
    }

    #[test]
    fn unknown_fields_inside_tagged_arms_are_rejected() {
        let (session, _run, _agent, mut message) = running_session();
        message.parts = vec![MessagePart::Text {
            text: "strict".to_owned(),
        }];
        let mut item = encode_domain_message(&message, session.workspace, session.organization)
            .expect("message");
        let document = item
            .get(AUTHORITY_DOCUMENT)
            .and_then(|value| value.as_s().ok())
            .expect("document");
        let mut value: serde_json::Value = serde_json::from_str(document).expect("json");
        value["parts"][0]["surprise"] = serde_json::Value::Bool(true);
        item.insert(
            AUTHORITY_DOCUMENT.to_owned(),
            AttributeValue::S(serde_json::to_string(&value).expect("json")),
        );
        assert!(decode_domain_message(&item, session.workspace).is_err());
    }
}
