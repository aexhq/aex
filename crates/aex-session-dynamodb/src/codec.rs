//! `session-authority` row codecs.
//!
//! Encoding and decoding are written as one pair per item type so a field can
//! never be written by one path and read by another. Every decode is total:
//! it either produces the value or a [`CodecError`], and there is no arm that
//! substitutes a default for something the authority is supposed to know.

use aex_operation_domain::{
    ContinuationCursor, FailureClass, Operation, OperationFailure, OperationKind, OperationResult,
    OperationScope, OperationStatus, Progress,
};
use aex_wire::CanonicalJson;
use aex_wire::error::ErrorCode;
use aex_wire::ids::{
    AgentId, ApprovalId, ContentHash, GenerationId, MeasurementId, MessageId, OperationId,
    OrganizationId, ResourceName, RunId, SessionId, ToolCallId, WorkspaceId,
};

use crate::attr::{CodecError, Item, ItemBuilder, Row, b, boolean, n, s, stamp};
use crate::keys;
use crate::replay::{Receipt, parse_intent};
use crate::wire_pending::{
    AgentControl, Approval, ApprovalBinding, ApprovalCancelCause, ApprovalStatus, Body,
    JournalEntry, Message, Run, SessionEvent, SessionHead, SessionLifecycle, SessionStatus,
    StoredOperation,
};

/// The `itemType` of a session head.
pub const SESSION_HEAD: &str = "session_head";
/// The `itemType` of a message.
pub const MESSAGE: &str = "message";
/// The `itemType` of a run.
pub const RUN: &str = "run";
pub use crate::event::{BODY_DIGEST, BODY_INLINE, SESSION_EVENT};
/// The `itemType` of an agent control item.
pub const AGENT_CONTROL: &str = "agent_control";
/// The `itemType` of a journal entry.
pub const JOURNAL_ENTRY: &str = "journal_entry";
/// The `itemType` of a tool approval.
pub const APPROVAL: &str = "approval";
/// The `itemType` of a durable operation.
pub const OPERATION: &str = "operation";
/// The `itemType` of an idempotency receipt.
pub const IDEMPOTENCY_RECEIPT: &str = "idempotency_receipt";
/// The `itemType` of an agent registry entry.
pub const AGENT_INDEX: &str = "agent_index";
/// The `itemType` of a spend reservation.
pub const SPEND_RESERVATION: &str = "spend_reservation";
/// The `itemType` of a fanout page.
pub const FANOUT_PAGE: &str = "fanout_page";
/// The `itemType` of an agent effect.
pub const AGENT_EFFECT: &str = "agent_effect";

/// The attribute a message's inline bytes live under.
pub const CONTENT_INLINE: &str = "contentInline";
/// The attribute a message's content reference lives under.
pub const CONTENT_DIGEST: &str = "contentDigest";

fn body_attributes(item: ItemBuilder, inline: &str, digest: &str, body: &Body) -> ItemBuilder {
    match body {
        Body::Inline(bytes) => item.set(inline, b(bytes.clone())),
        Body::Digest(reference) => item.set(digest, s(reference.clone())),
    }
}

fn read_body(
    row: &Row<'_>,
    inline: &'static str,
    digest: &'static str,
) -> Result<Body, CodecError> {
    if let Some(bytes) = row.opt_bytes(inline)? {
        return Ok(Body::Inline(bytes.to_vec()));
    }
    if let Some(reference) = row.opt_string(digest)? {
        return Ok(Body::Digest(reference.to_owned()));
    }
    Err(CodecError::Missing {
        item_type: SESSION_EVENT,
        attribute: inline,
    })
}

/// Encodes a session head, including its sparse index attributes.
///
/// A purged head carries neither index attribute, so it is physically absent
/// from the workspace index rather than filtered out of it.
#[must_use]
pub fn encode_head(head: &SessionHead) -> Item {
    let key = keys::head(head.session);
    let indexed = head.lifecycle != SessionLifecycle::Purged;
    let builder = ItemBuilder::new(SESSION_HEAD)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("sessionId", s(head.session.to_string()))
        .set("workspaceId", s(head.workspace.to_string()))
        .set("organizationId", s(head.organization.to_string()))
        .set("status", s(head.status.as_str()))
        .set("lifecycle", s(head.lifecycle.as_str()))
        .set("revision", n(head.revision))
        .set("deletionEpoch", n(head.deletion_epoch))
        .set("cancelEpoch", n(head.cancel_epoch))
        .set("contentAdmissionEpoch", n(head.content_admission_epoch))
        .set_opt("activeRunId", head.active_run.map(|run| s(run.to_string())))
        .set("rootAgentId", s(head.root_agent.to_string()))
        .set("agentBudget", n(head.agent_budget))
        .set(
            "resolvedConfigDigest",
            s(head.resolved_config_digest.clone()),
        )
        .set("custodyRevision", n(head.custody_revision))
        .set("createdAt", stamp(head.created_at))
        .set("updatedAt", stamp(head.updated_at))
        .set_opt("trashedAt", head.trashed_at.map(stamp))
        .set_opt("purgedAt", head.purged_at.map(stamp))
        .set_opt(
            "deletionOperationId",
            head.deletion_operation
                .map(|operation| s(operation.to_string())),
        );
    let builder = if indexed {
        builder
            .set(
                keys::workspace_index::PK,
                s(keys::workspace_index::session_partition(
                    head.workspace,
                    head.lifecycle.as_str(),
                )),
            )
            .set(
                keys::workspace_index::SK,
                s(keys::workspace_index::session_sort(
                    head.created_at,
                    head.session,
                )),
            )
    } else {
        builder
    };
    builder.build()
}

/// Decodes a session head and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] for any missing, mistyped or out-of-vocabulary attribute, and
/// [`CodecError::WrongTenant`] when the row belongs to another workspace.
pub fn decode_head(item: &Item, asserted: WorkspaceId) -> Result<SessionHead, CodecError> {
    let row = Row::bind(item, SESSION_HEAD)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let lifecycle_text = row.enumerated("lifecycle", keys::LIFECYCLES)?;
    let status_text = row.enumerated("status", keys::STATUSES)?;
    Ok(SessionHead {
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        organization: row.id::<OrganizationId>("organizationId")?,
        status: SessionStatus::parse(status_text).ok_or(CodecError::Malformed {
            item_type: SESSION_HEAD,
            attribute: "status",
            reason: "outside the closed status vocabulary".to_owned(),
        })?,
        lifecycle: SessionLifecycle::parse(lifecycle_text).ok_or(CodecError::Malformed {
            item_type: SESSION_HEAD,
            attribute: "lifecycle",
            reason: "outside the closed lifecycle vocabulary".to_owned(),
        })?,
        revision: row.u64("revision")?,
        deletion_epoch: row.u64("deletionEpoch")?,
        cancel_epoch: row.u64("cancelEpoch")?,
        content_admission_epoch: row.u64("contentAdmissionEpoch")?,
        active_run: row.opt_id::<RunId>("activeRunId")?,
        root_agent: row.id::<AgentId>("rootAgentId")?,
        agent_budget: row.u64("agentBudget")?,
        resolved_config_digest: row.string("resolvedConfigDigest")?.to_owned(),
        custody_revision: row.u64("custodyRevision")?,
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
        trashed_at: row.opt_timestamp("trashedAt")?,
        purged_at: row.opt_timestamp("purgedAt")?,
        deletion_operation: row.opt_id::<OperationId>("deletionOperationId")?,
    })
}

/// Encodes one message.
#[must_use]
pub fn encode_message(
    message: &Message,
    workspace: WorkspaceId,
    organization: OrganizationId,
) -> Item {
    let key = keys::message(message.session, message.message);
    body_attributes(
        ItemBuilder::new(MESSAGE)
            .set(crate::attr::PK, s(key.pk))
            .set(crate::attr::SK, s(key.sk))
            .set("messageId", s(message.message.to_string()))
            .set("sessionId", s(message.session.to_string()))
            .set("workspaceId", s(workspace.to_string()))
            .set("organizationId", s(organization.to_string()))
            .set_opt("runId", message.run.map(|run| s(run.to_string())))
            .set("role", s(message.role.clone()))
            .set("contentBytes", n(message.content_bytes))
            .set("createdAt", stamp(message.created_at)),
        CONTENT_INLINE,
        CONTENT_DIGEST,
        &message.body,
    )
    .build()
}

/// Decodes one message.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_message(item: &Item, asserted: WorkspaceId) -> Result<Message, CodecError> {
    let row = Row::bind(item, MESSAGE)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(Message {
        message: row.id::<MessageId>("messageId")?,
        session: row.id::<SessionId>("sessionId")?,
        run: row.opt_id::<RunId>("runId")?,
        role: row.string("role")?.to_owned(),
        body: read_body(&row, CONTENT_INLINE, CONTENT_DIGEST)?,
        content_bytes: row.u64("contentBytes")?,
        created_at: row.timestamp("createdAt")?,
    })
}

/// Encodes one run.
#[must_use]
pub fn encode_run(run: &Run, workspace: WorkspaceId, organization: OrganizationId) -> Item {
    let key = keys::run(run.session, run.run);
    ItemBuilder::new(RUN)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("runId", s(run.run.to_string()))
        .set("sessionId", s(run.session.to_string()))
        .set("workspaceId", s(workspace.to_string()))
        .set("organizationId", s(organization.to_string()))
        .set("messageId", s(run.message.to_string()))
        .set("status", s(run.status))
        .set("maxSpendCents", n(run.max_spend_cents))
        .set("reservationId", s(run.reservation.clone()))
        .set("deadlineAt", stamp(run.deadline_at))
        .set("queuedAt", stamp(run.queued_at))
        .set_opt("startedAt", run.started_at.map(stamp))
        .set_opt("terminalAt", run.terminal_at.map(stamp))
        .set_opt(
            "resultDigest",
            run.result_digest.as_ref().map(|digest| s(digest.clone())),
        )
        .build()
}

/// Decodes one run.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_run(item: &Item, asserted: WorkspaceId) -> Result<Run, CodecError> {
    let row = Row::bind(item, RUN)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(Run {
        run: row.id::<RunId>("runId")?,
        session: row.id::<SessionId>("sessionId")?,
        message: row.id::<MessageId>("messageId")?,
        status: row.enumerated("status", keys::RUN_STATUSES)?,
        max_spend_cents: row.u64("maxSpendCents")?,
        reservation: row.string("reservationId")?.to_owned(),
        deadline_at: row.timestamp("deadlineAt")?,
        queued_at: row.timestamp("queuedAt")?,
        started_at: row.opt_timestamp("startedAt")?,
        terminal_at: row.opt_timestamp("terminalAt")?,
        result_digest: row.opt_string("resultDigest")?.map(str::to_owned),
    })
}

/// Encodes one native event.
#[must_use]
pub fn encode_event(session: SessionId, event: &SessionEvent) -> Item {
    let key = keys::event(session, event.event_seq);
    body_attributes(
        ItemBuilder::new(SESSION_EVENT)
            .set(crate::attr::PK, s(key.pk))
            .set(crate::attr::SK, s(key.sk))
            .set("eventSeq", n(event.event_seq))
            .set("eventId", s(event.event_id.to_string()))
            .set("workspaceId", s(event.workspace.to_string()))
            .set("sessionId", s(session.to_string()))
            .set(
                crate::stream_keys::WORKSPACE_EVENT_PK,
                s(crate::stream_keys::workspace_event_partition(
                    event.workspace,
                    event.occurred_at,
                )),
            )
            .set(
                crate::stream_keys::WORKSPACE_EVENT_SK,
                s(crate::stream_keys::workspace_event_sort(
                    session,
                    event.event_id,
                    event.occurred_at,
                )),
            )
            .set("type", s(event.event_type.clone()))
            .set_opt("runId", event.run.map(|run| s(run.to_string())))
            .set_opt("agentId", event.agent.map(|agent| s(agent.to_string())))
            .set("occurredAt", stamp(event.occurred_at))
            .set("outboxState", s(event.outbox_state)),
        BODY_INLINE,
        BODY_DIGEST,
        &event.body,
    )
    .build()
}

/// Decodes one native event.
///
/// # Errors
///
/// [`CodecError`] as for every decode here, plus a rejection of an `eventId`
/// that is not `obs_`-prefixed — the observation stream reads this row as the
/// one authority for events, so the identity it will publish is checked here.
pub fn decode_event(item: &Item) -> Result<SessionEvent, CodecError> {
    crate::event::decode(item)
}

/// Encodes one agent control item.
#[must_use]
pub fn encode_control(control: &AgentControl) -> Item {
    let key = keys::agent_control(control.session, control.agent);
    ItemBuilder::new(AGENT_CONTROL)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("agentId", s(control.agent.to_string()))
        .set("sessionId", s(control.session.to_string()))
        .set("workspaceId", s(control.workspace.to_string()))
        .set("generationId", s(control.generation.clone()))
        .set("status", s(control.status.clone()))
        .set("revision", n(control.revision))
        .set("journalTail", n(control.journal_tail))
        .set_opt(
            "claimOwner",
            control.claim_owner.as_ref().map(|owner| s(owner.clone())),
        )
        .set_opt("leaseExpiresAt", control.lease_expires_at.map(stamp))
        .set("fence", n(control.fence))
        .set("childBudgetRemaining", n(control.child_budget_remaining))
        .set("childBudgetGranted", n(control.child_budget_granted))
        .set("createdAt", stamp(control.created_at))
        .set("updatedAt", stamp(control.updated_at))
        .build()
}

/// Decodes one agent control item.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_control(item: &Item, asserted: WorkspaceId) -> Result<AgentControl, CodecError> {
    let row = Row::bind(item, AGENT_CONTROL)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    Ok(AgentControl {
        agent: row.id::<AgentId>("agentId")?,
        session: row.id::<SessionId>("sessionId")?,
        workspace: asserted,
        generation: row.string("generationId")?.to_owned(),
        revision: row.u64("revision")?,
        journal_tail: row.u64("journalTail")?,
        claim_owner: row.opt_string("claimOwner")?.map(str::to_owned),
        lease_expires_at: row.opt_timestamp("leaseExpiresAt")?,
        fence: row.u64("fence")?,
        child_budget_remaining: row.u64("childBudgetRemaining")?,
        child_budget_granted: row.u64("childBudgetGranted")?,
        status: row.string("status")?.to_owned(),
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

/// Encodes one immutable journal entry.
#[must_use]
pub fn encode_journal(session: SessionId, agent: AgentId, entry: &JournalEntry) -> Item {
    let key = keys::journal(session, agent, entry.seq);
    body_attributes(
        ItemBuilder::new(JOURNAL_ENTRY)
            .set(crate::attr::PK, s(key.pk))
            .set(crate::attr::SK, s(key.sk))
            .set("seq", n(entry.seq))
            .set("entryId", s(entry.entry_id.clone()))
            .set("kind", s(entry.kind.clone()))
            .set("bodyBytes", n(entry.body_bytes))
            .set("occurredAt", stamp(entry.occurred_at)),
        BODY_INLINE,
        BODY_DIGEST,
        &entry.body,
    )
    .build()
}

/// Decodes one journal entry.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_journal(item: &Item) -> Result<JournalEntry, CodecError> {
    let row = Row::bind(item, JOURNAL_ENTRY)?;
    Ok(JournalEntry {
        seq: row.u64("seq")?,
        entry_id: row.string("entryId")?.to_owned(),
        kind: row.string("kind")?.to_owned(),
        body: read_body(&row, BODY_INLINE, BODY_DIGEST)?,
        body_bytes: row.u64("bodyBytes")?,
        occurred_at: row.timestamp("occurredAt")?,
    })
}

/// Reads a `sha256:<hex>` digest attribute.
fn digest(row: &Row<'_>, attribute: &'static str) -> Result<ContentHash, CodecError> {
    ContentHash::parse(row.string(attribute)?).map_err(|error| CodecError::Malformed {
        item_type: APPROVAL,
        attribute,
        reason: error.to_string(),
    })
}

/// Encodes one tool approval.
///
/// All eleven bound fields are written. The wire publishes seven of them; the
/// other four are what `respond` revalidates against, and an approval that
/// could not be revalidated would have to be dispatched on trust.
#[must_use]
pub fn encode_approval(approval: &Approval) -> Item {
    let key = keys::approval(approval.binding.session, approval.approval);
    ItemBuilder::new(APPROVAL)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("approvalId", s(approval.approval.to_string()))
        .set("workspaceId", s(approval.workspace.to_string()))
        .set("sessionId", s(approval.binding.session.to_string()))
        .set("runId", s(approval.binding.run.to_string()))
        .set("agentId", s(approval.binding.agent.to_string()))
        .set("toolCallId", s(approval.binding.tool_call.to_string()))
        .set("toolName", s(approval.binding.tool.to_string()))
        .set(
            "argumentDigest",
            s(approval.binding.argument_digest.to_wire()),
        )
        .set(
            "implementationDigest",
            s(approval.binding.implementation_digest.to_wire()),
        )
        .set("configDigest", s(approval.binding.config_digest.to_wire()))
        .set_opt(
            "expectedGenerationId",
            approval
                .binding
                .expected_generation
                .map(|generation| s(generation.to_string())),
        )
        .set(
            "expectedCustodyRevision",
            n(approval.binding.expected_custody),
        )
        .set(
            "expectedConfigRevision",
            n(approval.binding.expected_config_revision),
        )
        .set("status", s(approval.status.as_str()))
        .set_opt(
            "cancelCause",
            approval.cancel_cause.map(|cause| s(cause.as_str())),
        )
        .set("createdAt", stamp(approval.created_at))
        .set("expiresAt", stamp(approval.expires_at))
        .set_opt("resolvedAt", approval.resolved_at.map(stamp))
        .build()
}

/// Decodes one tool approval and re-checks its ownership.
///
/// # Errors
///
/// [`CodecError`] as for every decode here. A status or cancel cause outside its
/// closed vocabulary is a refusal rather than a guess: an approval read as the
/// wrong state decides whether a tool call runs.
pub fn decode_approval(item: &Item, asserted: WorkspaceId) -> Result<Approval, CodecError> {
    let row = Row::bind(item, APPROVAL)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let status_text = row.enumerated("status", keys::APPROVAL_STATUSES)?;
    let status = ApprovalStatus::parse(status_text).ok_or(CodecError::Malformed {
        item_type: APPROVAL,
        attribute: "status",
        reason: "outside the closed approval status vocabulary".to_owned(),
    })?;
    let cancel_cause = match row.opt_string("cancelCause")? {
        None => None,
        Some(stored) => Some(
            ApprovalCancelCause::parse(stored).ok_or(CodecError::Malformed {
                item_type: APPROVAL,
                attribute: "cancelCause",
                reason: "outside the closed cancel-cause vocabulary".to_owned(),
            })?,
        ),
    };
    let tool =
        ResourceName::parse(row.string("toolName")?).map_err(|error| CodecError::Malformed {
            item_type: APPROVAL,
            attribute: "toolName",
            reason: error.to_string(),
        })?;
    Ok(Approval {
        approval: row.id::<ApprovalId>("approvalId")?,
        workspace: asserted,
        binding: ApprovalBinding {
            session: row.id::<SessionId>("sessionId")?,
            run: row.id::<RunId>("runId")?,
            agent: row.id::<AgentId>("agentId")?,
            tool_call: row.id::<ToolCallId>("toolCallId")?,
            tool,
            argument_digest: digest(&row, "argumentDigest")?,
            implementation_digest: digest(&row, "implementationDigest")?,
            config_digest: digest(&row, "configDigest")?,
            expected_generation: row.opt_id::<GenerationId>("expectedGenerationId")?,
            expected_custody: row.u64("expectedCustodyRevision")?,
            expected_config_revision: row.u64("expectedConfigRevision")?,
        },
        status,
        cancel_cause,
        created_at: row.timestamp("createdAt")?,
        expires_at: row.timestamp("expiresAt")?,
        resolved_at: row.opt_timestamp("resolvedAt")?,
    })
}

/// Encodes one durable operation record without discarding domain fields.
///
/// # Errors
///
/// [`CodecError::Malformed`] if a caller mutated a continuation cursor after
/// construction so its canonical encoding is no longer valid.
#[allow(
    clippy::too_many_lines,
    reason = "one linear row encoder keeps every persisted operation field visible together"
)]
pub fn encode_operation(operation: &StoredOperation) -> Result<Item, CodecError> {
    let record = &operation.record;
    let key = keys::operation(record.id);
    let (scope_kind, scope_id) = match record.scope {
        OperationScope::Session(session) => ("session", session.to_string()),
        OperationScope::Workspace(workspace) => ("workspace", workspace.to_string()),
    };
    let cursor = record
        .cursor
        .as_ref()
        .map(ContinuationCursor::encode)
        .transpose()
        .map_err(|error| CodecError::Malformed {
            item_type: OPERATION,
            attribute: "continuationCursor",
            reason: error.to_string(),
        })?;
    let mut item = ItemBuilder::new(OPERATION)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("operationId", s(record.id.to_string()))
        .set("workspaceId", s(record.workspace.to_string()))
        .set_opt(
            "sessionId",
            record.session.map(|session| s(session.to_string())),
        )
        .set("kind", s(record.kind.as_str()))
        .set("status", s(record.status.as_str()))
        .set("intentHash", s(record.intent.to_string()))
        .set("scopeKind", s(scope_kind))
        .set("scopeId", s(scope_id))
        .set("version", n(operation.version))
        .set(
            "claimsSessionDeletion",
            boolean(record.kind.claims_session_deletion()),
        )
        .set("cancelRequested", boolean(record.cancel_requested))
        .set_opt(
            "progressPhase",
            record.progress.as_ref().map(|progress| s(&progress.phase)),
        )
        .set_opt(
            "progressProcessed",
            record
                .progress
                .as_ref()
                .map(|progress| n(progress.processed)),
        )
        .set_opt(
            "progressTotal",
            record
                .progress
                .as_ref()
                .and_then(|progress| progress.total_hint)
                .map(n),
        )
        .set_opt("continuationCursor", cursor.map(b))
        .set("resultPresent", boolean(record.result.is_some()))
        .set_opt(
            "resultMeasurementId",
            record
                .result
                .as_ref()
                .and_then(|result| result.measurement)
                .map(|measurement| s(measurement.to_string())),
        )
        .set_opt(
            "resultJson",
            record
                .result
                .as_ref()
                .and_then(|result| result.content.as_ref())
                .map(|content| s(content.as_str())),
        )
        .set_opt(
            "errorCode",
            record.error.as_ref().map(|error| s(error.code.as_str())),
        )
        .set_opt(
            "errorClass",
            record.error.as_ref().map(|error| s(error.class.as_str())),
        )
        .set_opt(
            "errorDetailJson",
            record
                .error
                .as_ref()
                .and_then(|error| error.detail.as_ref())
                .map(|detail| s(detail.as_str())),
        )
        .set("createdAt", stamp(record.created_at))
        .set_opt("startedAt", record.started_at.map(stamp))
        .set("updatedAt", stamp(record.updated_at))
        .set_opt("committedAt", record.committed_at.map(stamp))
        .set_opt("terminalAt", record.terminal_at.map(stamp));
    if record.kind.is_public() {
        item = item
            .set(
                keys::workspace_index::PK,
                s(keys::workspace_index::operation_partition(record.workspace)),
            )
            .set(
                keys::workspace_index::SK,
                s(keys::workspace_index::operation_sort(
                    record.created_at,
                    record.id,
                )),
            );
    }
    Ok(item.build())
}

/// Decodes one durable operation record.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
#[allow(
    clippy::too_many_lines,
    reason = "one strict row decoder keeps every operation invariant in a single audit surface"
)]
pub fn decode_operation(item: &Item, asserted: WorkspaceId) -> Result<StoredOperation, CodecError> {
    let row = Row::bind(item, OPERATION)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let intent_hex = row.string("intentHash")?;
    let intent = parse_intent(intent_hex).ok_or(CodecError::Malformed {
        item_type: OPERATION,
        attribute: "intentHash",
        reason: "an intent hash is 64 lowercase hex characters".to_owned(),
    })?;
    let operation = row.id::<OperationId>("operationId")?;
    let session = row.opt_id::<SessionId>("sessionId")?;
    let kind = OperationKind::parse(row.string("kind")?).ok_or_else(|| CodecError::Malformed {
        item_type: OPERATION,
        attribute: "kind",
        reason: "outside the closed operation-kind vocabulary".to_owned(),
    })?;
    let status =
        OperationStatus::parse(row.string("status")?).ok_or_else(|| CodecError::Malformed {
            item_type: OPERATION,
            attribute: "status",
            reason: "outside the closed operation-status vocabulary".to_owned(),
        })?;
    let scope = match row.string("scopeKind")? {
        "session" => OperationScope::Session(row.id::<SessionId>("scopeId")?),
        "workspace" => OperationScope::Workspace(row.id::<WorkspaceId>("scopeId")?),
        _ => {
            return Err(CodecError::Malformed {
                item_type: OPERATION,
                attribute: "scopeKind",
                reason: "expected `session` or `workspace`".to_owned(),
            });
        }
    };
    let progress = match row.opt_string("progressPhase")? {
        None => None,
        Some(phase) => {
            let progress = Progress {
                phase: phase.to_owned(),
                processed: row.u64("progressProcessed")?,
                total_hint: row.opt_u64("progressTotal")?,
            };
            progress.validate().map_err(|error| CodecError::Malformed {
                item_type: OPERATION,
                attribute: "progressPhase",
                reason: error.to_string(),
            })?;
            Some(progress)
        }
    };
    let cursor = row
        .opt_bytes("continuationCursor")?
        .map(ContinuationCursor::decode)
        .transpose()
        .map_err(|error| CodecError::Malformed {
            item_type: OPERATION,
            attribute: "continuationCursor",
            reason: error.to_string(),
        })?;
    let result = if row.boolean("resultPresent")? {
        Some(OperationResult {
            measurement: row.opt_id::<MeasurementId>("resultMeasurementId")?,
            content: row
                .opt_string("resultJson")?
                .map(CanonicalJson::parse)
                .transpose()
                .map_err(|error| CodecError::Malformed {
                    item_type: OPERATION,
                    attribute: "resultJson",
                    reason: error.to_string(),
                })?,
        })
    } else {
        None
    };
    let error = match row.opt_string("errorCode")? {
        None => None,
        Some(code) => Some(OperationFailure {
            code: ErrorCode::parse(code).ok_or_else(|| CodecError::Malformed {
                item_type: OPERATION,
                attribute: "errorCode",
                reason: "outside the closed public error-code vocabulary".to_owned(),
            })?,
            class: FailureClass::parse(row.string("errorClass")?).ok_or_else(|| {
                CodecError::Malformed {
                    item_type: OPERATION,
                    attribute: "errorClass",
                    reason: "outside the closed failure-class vocabulary".to_owned(),
                }
            })?,
            detail: row
                .opt_string("errorDetailJson")?
                .map(CanonicalJson::parse)
                .transpose()
                .map_err(|error| CodecError::Malformed {
                    item_type: OPERATION,
                    attribute: "errorDetailJson",
                    reason: error.to_string(),
                })?,
        }),
    };
    let stored_claim = row.boolean("claimsSessionDeletion")?;
    if stored_claim != kind.claims_session_deletion() {
        return Err(CodecError::Malformed {
            item_type: OPERATION,
            attribute: "claimsSessionDeletion",
            reason: "does not match the operation kind".to_owned(),
        });
    }
    Ok(StoredOperation {
        record: Operation {
            id: operation,
            workspace: asserted,
            session,
            kind,
            status,
            intent,
            scope,
            progress,
            cursor,
            cancel_requested: row.boolean("cancelRequested")?,
            result,
            error,
            created_at: row.timestamp("createdAt")?,
            started_at: row.opt_timestamp("startedAt")?,
            updated_at: row.timestamp("updatedAt")?,
            committed_at: row.opt_timestamp("committedAt")?,
            terminal_at: row.opt_timestamp("terminalAt")?,
        },
        version: row.u64("version")?,
    })
}

/// Encodes one idempotency receipt.
///
/// The receipt row shape is shared by every regional table that holds one, so
/// the codec lives in [`crate::replay`] and this is the `session-authority`
/// spelling of it. A second implementation would be a second, subtly different
/// idempotency guarantee.
///
/// # Errors
///
/// [`crate::component::KeyError`] when the rendered scope or the key digest
/// could not enter a key.
pub fn encode_receipt(
    workspace: WorkspaceId,
    receipt: &Receipt,
) -> Result<Item, crate::component::KeyError> {
    crate::replay::encode_receipt_row(workspace, receipt)
}

/// Decodes one idempotency receipt.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_receipt(item: &Item) -> Result<Receipt, CodecError> {
    crate::replay::decode_receipt_row(item)
}

pub use crate::replay::receipt_is_live;

#[cfg(test)]
mod tests {
    use aex_operation_domain::{
        FailureClass, Operation, OperationFailure, OperationKind, OperationResult, OperationScope,
        OperationStatus, Progress,
    };
    use aex_wire::CanonicalJson;
    use aex_wire::error::ErrorCode;
    use aex_wire::idempotency::IntentDigest;
    use aex_wire::ids::{
        AgentId, MessageId, ObservationId, OperationId, OrganizationId, PrefixedId, RunId,
        SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::Timestamp;

    use super::{
        SESSION_HEAD, decode_approval, decode_event, decode_head, decode_message, decode_operation,
        decode_run, encode_approval, encode_event, encode_head, encode_message, encode_operation,
        encode_run,
    };
    use crate::attr::CodecError;
    use crate::keys;
    use crate::wire_pending::{
        Approval, ApprovalBinding, ApprovalCancelCause, ApprovalStatus, Body, Message, Run,
        SessionEvent, SessionHead, SessionLifecycle, SessionStatus, StoredOperation,
    };

    fn stamp(millis: i64) -> Timestamp {
        Timestamp::from_unix_millis(millis).expect("in range")
    }

    fn workspace(byte: u8) -> WorkspaceId {
        WorkspaceId::from_uuid7(Uuid7::compose(1, [byte; 10]))
    }

    fn head() -> SessionHead {
        SessionHead {
            session: SessionId::from_uuid7(Uuid7::compose(1_000, [1; 10])),
            workspace: workspace(1),
            organization: OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10])),
            status: SessionStatus::Idle,
            lifecycle: SessionLifecycle::Active,
            revision: 3,
            deletion_epoch: 0,
            cancel_epoch: 0,
            content_admission_epoch: 0,
            active_run: None,
            root_agent: AgentId::from_uuid7(Uuid7::compose(1, [3; 10])),
            agent_budget: 256,
            resolved_config_digest: "sha256:".to_owned() + &"a".repeat(64),
            custody_revision: 1,
            created_at: stamp(1_000),
            updated_at: stamp(2_000),
            trashed_at: None,
            purged_at: None,
            deletion_operation: None,
        }
    }

    fn stored_operation(kind: OperationKind) -> StoredOperation {
        let workspace = workspace(1);
        StoredOperation {
            record: Operation {
                id: OperationId::from_uuid7(Uuid7::compose(1_000, [8; 10])),
                workspace,
                session: None,
                kind,
                status: OperationStatus::Succeeded,
                intent: IntentDigest::from_bytes([9; 32]),
                scope: OperationScope::Workspace(workspace),
                progress: Some(Progress {
                    phase: "finalizing".to_owned(),
                    processed: 3,
                    total_hint: Some(3),
                }),
                cursor: None,
                cancel_requested: false,
                result: Some(OperationResult {
                    measurement: None,
                    content: Some(
                        CanonicalJson::parse(r#"{"changed":true}"#).expect("canonical json"),
                    ),
                }),
                error: None,
                created_at: stamp(1_000),
                started_at: Some(stamp(1_100)),
                updated_at: stamp(2_000),
                committed_at: Some(stamp(1_900)),
                terminal_at: Some(stamp(2_000)),
            },
            version: 4,
        }
    }

    #[test]
    fn an_operation_round_trips_without_losing_public_projection_fields() {
        let original = stored_operation(OperationKind::SessionStop);
        let item = encode_operation(&original).expect("encodes");
        let decoded = decode_operation(&item, original.record.workspace).expect("decodes");
        assert_eq!(decoded, original);

        let mut failed = stored_operation(OperationKind::TelemetryExport);
        failed.record.status = OperationStatus::Failed;
        failed.record.result = None;
        failed.record.error = Some(OperationFailure {
            code: ErrorCode::UpstreamError,
            class: FailureClass::Retryable,
            detail: Some(CanonicalJson::parse(r#"{"provider":"fixture"}"#).expect("json")),
        });
        let item = encode_operation(&failed).expect("encodes");
        assert_eq!(
            decode_operation(&item, failed.record.workspace).expect("decodes"),
            failed
        );
    }

    #[test]
    fn content_gc_never_enters_the_public_workspace_operation_index() {
        let internal = stored_operation(OperationKind::ContentGc);
        let item = encode_operation(&internal).expect("encodes");
        assert!(!item.contains_key(keys::workspace_index::PK));
        assert!(!item.contains_key(keys::workspace_index::SK));

        let public = stored_operation(OperationKind::WorkspaceDelete);
        let item = encode_operation(&public).expect("encodes");
        assert!(item.contains_key(keys::workspace_index::PK));
        assert!(item.contains_key(keys::workspace_index::SK));
    }

    #[test]
    fn a_head_round_trips_exactly() {
        let original = head();
        let decoded = decode_head(&encode_head(&original), original.workspace).expect("decodes");
        assert_eq!(decoded, original);
    }

    #[test]
    fn a_head_from_another_workspace_is_rejected_after_read() {
        let original = head();
        let error =
            decode_head(&encode_head(&original), workspace(9)).expect_err("another workspace");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }

    #[test]
    fn an_active_head_carries_the_index_attributes_and_a_purged_one_does_not() {
        let mut active = head();
        active.lifecycle = SessionLifecycle::Active;
        let encoded = encode_head(&active);
        assert!(encoded.contains_key(keys::workspace_index::PK));
        assert!(encoded.contains_key(keys::workspace_index::SK));

        let mut purged = head();
        purged.lifecycle = SessionLifecycle::Purged;
        purged.purged_at = Some(stamp(9_000));
        let encoded = encode_head(&purged);
        assert!(
            !encoded.contains_key(keys::workspace_index::PK),
            "a purged head must be physically absent from the workspace index"
        );
        assert!(!encoded.contains_key(keys::workspace_index::SK));
    }

    #[test]
    fn trashing_moves_the_head_to_a_different_index_partition() {
        let mut active = head();
        active.lifecycle = SessionLifecycle::Active;
        let mut trashed = head();
        trashed.lifecycle = SessionLifecycle::Trashed;
        assert_ne!(
            encode_head(&active)[keys::workspace_index::PK],
            encode_head(&trashed)[keys::workspace_index::PK]
        );
    }

    #[test]
    fn a_head_decoded_as_another_item_type_is_rejected() {
        let encoded = encode_head(&head());
        let error = crate::attr::Row::bind(&encoded, "run").expect_err("not a run");
        assert!(
            matches!(
                error,
                CodecError::UnexpectedItemType {
                    expected: "run",
                    ref found
                } if found == SESSION_HEAD
            ),
            "{error}"
        );
    }

    #[test]
    fn a_message_round_trips_with_an_inline_body_and_with_a_reference() {
        let base = Message {
            message: MessageId::from_uuid7(Uuid7::compose(1, [4; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            run: Some(RunId::from_uuid7(Uuid7::compose(1, [5; 10]))),
            role: "user".to_owned(),
            body: Body::Inline(b"hello".to_vec()),
            content_bytes: 5,
            created_at: stamp(1),
        };
        let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let encoded = encode_message(&base, workspace(1), organization);
        assert_eq!(
            decode_message(&encoded, workspace(1)).expect("decodes"),
            base
        );

        let referenced = Message {
            body: Body::Digest("sha256:".to_owned() + &"b".repeat(64)),
            ..base
        };
        let encoded = encode_message(&referenced, workspace(1), organization);
        assert_eq!(
            decode_message(&encoded, workspace(1)).expect("decodes"),
            referenced
        );
    }

    #[test]
    fn a_run_round_trips_and_rejects_a_status_outside_the_vocabulary() {
        let run = Run {
            run: RunId::from_uuid7(Uuid7::compose(1, [5; 10])),
            session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
            message: MessageId::from_uuid7(Uuid7::compose(1, [4; 10])),
            status: "queued",
            max_spend_cents: 500,
            reservation: "rsv-1".to_owned(),
            deadline_at: stamp(10_000),
            queued_at: stamp(1_000),
            started_at: None,
            terminal_at: None,
            result_digest: None,
        };
        let organization = OrganizationId::from_uuid7(Uuid7::compose(1, [2; 10]));
        let mut encoded = encode_run(&run, workspace(1), organization);
        assert_eq!(decode_run(&encoded, workspace(1)).expect("decodes"), run);

        encoded.insert("status".to_owned(), crate::attr::s("teleported"));
        assert!(matches!(
            decode_run(&encoded, workspace(1)),
            Err(CodecError::Malformed { .. })
        ));
    }

    fn approval() -> Approval {
        use aex_wire::ids::{ApprovalId, ContentHash, GenerationId, ResourceName, ToolCallId};

        Approval {
            approval: ApprovalId::from_uuid7(Uuid7::compose(1, [6; 10])),
            workspace: workspace(1),
            binding: ApprovalBinding {
                session: SessionId::from_uuid7(Uuid7::compose(1, [1; 10])),
                run: RunId::from_uuid7(Uuid7::compose(1, [5; 10])),
                agent: AgentId::from_uuid7(Uuid7::compose(1, [3; 10])),
                tool_call: ToolCallId::from_uuid7(Uuid7::compose(1, [7; 10])),
                tool: ResourceName::parse("web.fetch").expect("a resource name"),
                argument_digest: ContentHash::of(b"arguments"),
                implementation_digest: ContentHash::of(b"implementation"),
                config_digest: ContentHash::of(b"config"),
                expected_generation: Some(GenerationId::from_uuid7(Uuid7::compose(1, [8; 10]))),
                expected_custody: 4,
                expected_config_revision: 9,
            },
            status: ApprovalStatus::Pending,
            cancel_cause: None,
            created_at: stamp(1_000),
            expires_at: stamp(61_000),
            resolved_at: None,
        }
    }

    #[test]
    fn an_approval_round_trips_with_all_eleven_bound_fields() {
        let original = approval();
        let encoded = encode_approval(&original);
        assert_eq!(
            decode_approval(&encoded, original.workspace).expect("decodes"),
            original
        );

        // Every bound field the domain names reaches a stored attribute; a
        // binding that lost one could not be revalidated at decision time.
        for attribute in [
            "sessionId",
            "runId",
            "agentId",
            "toolCallId",
            "toolName",
            "argumentDigest",
            "implementationDigest",
            "configDigest",
            "expectedGenerationId",
            "expectedCustodyRevision",
            "expectedConfigRevision",
        ] {
            assert!(
                encoded.contains_key(attribute),
                "`{attribute}` is not stored"
            );
        }
    }

    #[test]
    fn a_withdrawn_approval_round_trips_with_its_cause() {
        let withdrawn = Approval {
            status: ApprovalStatus::Cancelled,
            cancel_cause: Some(ApprovalCancelCause::BindingDrift),
            resolved_at: Some(stamp(2_000)),
            ..approval()
        };
        assert_eq!(
            decode_approval(&encode_approval(&withdrawn), withdrawn.workspace).expect("decodes"),
            withdrawn
        );
    }

    #[test]
    fn an_expired_approval_round_trips_without_a_cancel_cause() {
        let expired = Approval {
            status: ApprovalStatus::Expired,
            cancel_cause: None,
            resolved_at: Some(stamp(61_000)),
            ..approval()
        };
        assert_eq!(
            decode_approval(&encode_approval(&expired), expired.workspace).expect("decodes"),
            expired
        );
    }

    #[test]
    fn an_approval_from_another_workspace_is_rejected_after_read() {
        let encoded = encode_approval(&approval());
        let error = decode_approval(&encoded, workspace(9)).expect_err("another workspace");
        assert!(matches!(error, CodecError::WrongTenant { .. }), "{error}");
    }

    #[test]
    fn an_approval_status_or_cause_outside_its_vocabulary_is_refused_rather_than_guessed() {
        let mut invented = encode_approval(&approval());
        invented.insert("status".to_owned(), crate::attr::s("maybe"));
        assert!(decode_approval(&invented, workspace(1)).is_err());

        let mut caused = encode_approval(&approval());
        caused.insert(
            "cancelCause".to_owned(),
            crate::attr::s("the moon was wrong"),
        );
        assert!(decode_approval(&caused, workspace(1)).is_err());
    }

    #[test]
    fn an_approval_that_names_no_expiry_is_refused_rather_than_read_as_open_ended() {
        // "When it stops being decidable" is a fact about the raised approval. A
        // reader that saw the attribute missing and carried on would publish an
        // approval that never expires.
        let mut endless = encode_approval(&approval());
        endless.remove("expiresAt");
        assert!(matches!(
            decode_approval(&endless, workspace(1)),
            Err(CodecError::Missing {
                attribute: "expiresAt",
                ..
            })
        ));
    }

    #[test]
    fn an_approval_lives_in_its_session_partition_under_the_listed_prefix() {
        let encoded = encode_approval(&approval());
        let partition = keys::session_partition(approval().binding.session);
        assert_eq!(encoded[crate::attr::PK], crate::attr::s(partition));
        let sort = encoded[crate::attr::SK]
            .as_s()
            .expect("a string sort key")
            .clone();
        assert!(
            sort.starts_with(keys::approval_prefix()),
            "`{sort}` is outside the listed range"
        );
    }

    #[test]
    fn an_event_identity_must_be_an_observation_id() {
        let event = SessionEvent {
            workspace: workspace(1),
            event_seq: 1,
            event_id: ObservationId::from_uuid7(Uuid7::compose(1, [5; 10])),
            event_type: "run.admitted".to_owned(),
            run: None,
            agent: None,
            body: Body::Inline(b"{}".to_vec()),
            occurred_at: stamp(1),
            outbox_state: "pending",
        };
        let session = SessionId::from_uuid7(Uuid7::compose(1, [1; 10]));
        let mut encoded = encode_event(session, &event);
        assert_eq!(decode_event(&encoded).expect("decodes"), event);
        assert!(
            encoded[crate::stream_keys::WORKSPACE_EVENT_PK]
                .as_s()
                .expect("workspace event partition")
                .starts_with(&format!("EVTW#{}#", event.workspace))
        );
        assert!(
            encoded[crate::stream_keys::WORKSPACE_EVENT_SK]
                .as_s()
                .expect("workspace event sort")
                .ends_with(&format!("#{session}#{}", event.event_id))
        );

        encoded.insert("eventId".to_owned(), crate::attr::s("evt_1"));
        let error = decode_event(&encoded).expect_err("not an observation id");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }
}
