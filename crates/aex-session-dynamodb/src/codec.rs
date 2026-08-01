//! `session-authority` row codecs.
//!
//! Encoding and decoding are written as one pair per item type so a field can
//! never be written by one path and read by another. Every decode is total:
//! it either produces the value or a [`CodecError`], and there is no arm that
//! substitutes a default for something the authority is supposed to know.

use aex_wire::idempotency::IntentDigest;
use aex_wire::ids::{
    AgentId, MessageId, OperationId, OrganizationId, RunId, SessionId, WorkspaceId,
};
use aex_wire::types::Timestamp;

use crate::attr::{CodecError, Item, ItemBuilder, Row, b, boolean, n, s, stamp};
use crate::keys;
use crate::replay::{Receipt, ReceiptBody};
use crate::wire_pending::{
    AgentControl, Body, JournalEntry, Message, Run, SessionEvent, SessionHead, SessionLifecycle,
    SessionStatus, StoredOperation,
};

/// The `itemType` of a session head.
pub const SESSION_HEAD: &str = "session_head";
/// The `itemType` of a message.
pub const MESSAGE: &str = "message";
/// The `itemType` of a run.
pub const RUN: &str = "run";
/// The `itemType` of a native event.
pub const SESSION_EVENT: &str = "session_event";
/// The `itemType` of an agent control item.
pub const AGENT_CONTROL: &str = "agent_control";
/// The `itemType` of a journal entry.
pub const JOURNAL_ENTRY: &str = "journal_entry";
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

/// The attribute a body's inline bytes live under.
pub const BODY_INLINE: &str = "bodyInline";
/// The attribute a body's content reference lives under.
pub const BODY_DIGEST: &str = "bodyDigest";
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
            .set("eventId", s(event.event_id.clone()))
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
    let row = Row::bind(item, SESSION_EVENT)?;
    let event_id = row.string("eventId")?;
    if !event_id.starts_with("obs_") {
        return Err(CodecError::Malformed {
            item_type: SESSION_EVENT,
            attribute: "eventId",
            reason: "a session event identity is an `obs_`-prefixed observation id".to_owned(),
        });
    }
    Ok(SessionEvent {
        event_seq: row.u64("eventSeq")?,
        event_id: event_id.to_owned(),
        event_type: row.string("type")?.to_owned(),
        run: row.opt_id::<RunId>("runId")?,
        agent: row.opt_id::<AgentId>("agentId")?,
        body: read_body(&row, BODY_INLINE, BODY_DIGEST)?,
        occurred_at: row.timestamp("occurredAt")?,
        outbox_state: row.enumerated("outboxState", keys::OUTBOX_STATES)?,
    })
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

/// Encodes one durable operation record.
#[must_use]
pub fn encode_operation(operation: &StoredOperation) -> Item {
    let key = keys::operation(operation.operation);
    ItemBuilder::new(OPERATION)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("operationId", s(operation.operation.to_string()))
        .set("workspaceId", s(operation.workspace.to_string()))
        .set_opt(
            "sessionId",
            operation.session.map(|session| s(session.to_string())),
        )
        .set("kind", s(operation.kind.clone()))
        .set("status", s(operation.status.clone()))
        .set("intentHash", s(operation.intent.to_string()))
        .set("version", n(operation.version))
        .set(
            "claimsSessionDeletion",
            boolean(operation.claims_session_deletion),
        )
        .set("createdAt", stamp(operation.created_at))
        .set("updatedAt", stamp(operation.updated_at))
        .set(
            keys::workspace_index::PK,
            s(keys::workspace_index::operation_partition(
                operation.workspace,
            )),
        )
        .set(
            keys::workspace_index::SK,
            s(keys::workspace_index::operation_sort(
                operation.created_at,
                operation.operation,
            )),
        )
        .build()
}

/// Decodes one durable operation record.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_operation(item: &Item, asserted: WorkspaceId) -> Result<StoredOperation, CodecError> {
    let row = Row::bind(item, OPERATION)?;
    row.owned_by("workspaceId", &asserted.to_string())?;
    let intent_hex = row.string("intentHash")?;
    let intent = parse_intent(intent_hex).ok_or(CodecError::Malformed {
        item_type: OPERATION,
        attribute: "intentHash",
        reason: "an intent hash is 64 lowercase hex characters".to_owned(),
    })?;
    Ok(StoredOperation {
        operation: row.id::<OperationId>("operationId")?,
        workspace: asserted,
        session: row.opt_id::<SessionId>("sessionId")?,
        kind: row.string("kind")?.to_owned(),
        status: row.string("status")?.to_owned(),
        intent,
        version: row.u64("version")?,
        claims_session_deletion: row.boolean("claimsSessionDeletion")?,
        created_at: row.timestamp("createdAt")?,
        updated_at: row.timestamp("updatedAt")?,
    })
}

fn parse_intent(text: &str) -> Option<IntentDigest> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        *slot = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(IntentDigest::from_bytes(bytes))
}

/// Encodes one idempotency receipt.
///
/// The row carries both the epoch-seconds TTL attribute and the explicit
/// `expiresAt` the reader checks. TTL reclaims space; it is never the fence
/// (D-24).
///
/// # Errors
///
/// [`crate::component::KeyError`] when the rendered scope or the key digest
/// could not enter a key.
pub fn encode_receipt(
    workspace: WorkspaceId,
    receipt: &Receipt,
) -> Result<Item, crate::component::KeyError> {
    let key = keys::receipt(workspace, &receipt.scope, &receipt.key_sha256)?;
    let builder = ItemBuilder::new(IDEMPOTENCY_RECEIPT)
        .set(crate::attr::PK, s(key.pk))
        .set(crate::attr::SK, s(key.sk))
        .set("scope", s(receipt.scope.clone()))
        .set("keySha256", s(receipt.key_sha256.clone()))
        .set("intentHash", s(receipt.intent.to_string()))
        .set("responseKind", s(receipt.response_kind.clone()))
        .set("committedAt", stamp(receipt.committed_at))
        .set("expiresAt", stamp(receipt.expires_at))
        .set(
            "expiresAtEpochSeconds",
            crate::attr::n_i64(receipt.expires_at.unix_millis().div_euclid(1_000)),
        );
    Ok(match &receipt.response {
        ReceiptBody::Inline(bytes) => builder.set("responseInline", b(bytes.clone())),
        ReceiptBody::Digest(digest) => builder.set("responseDigest", s(digest.clone())),
    }
    .build())
}

/// Decodes one idempotency receipt.
///
/// # Errors
///
/// [`CodecError`] as for every decode here.
pub fn decode_receipt(item: &Item) -> Result<Receipt, CodecError> {
    let row = Row::bind(item, IDEMPOTENCY_RECEIPT)?;
    let intent_hex = row.string("intentHash")?;
    let intent = parse_intent(intent_hex).ok_or(CodecError::Malformed {
        item_type: IDEMPOTENCY_RECEIPT,
        attribute: "intentHash",
        reason: "an intent hash is 64 lowercase hex characters".to_owned(),
    })?;
    let response = match (
        row.opt_bytes("responseInline")?,
        row.opt_string("responseDigest")?,
    ) {
        (Some(bytes), _) => ReceiptBody::Inline(bytes.to_vec()),
        (None, Some(digest)) => ReceiptBody::Digest(digest.to_owned()),
        (None, None) => {
            return Err(CodecError::Missing {
                item_type: IDEMPOTENCY_RECEIPT,
                attribute: "responseInline",
            });
        }
    };
    Ok(Receipt {
        scope: row.string("scope")?.to_owned(),
        key_sha256: row.string("keySha256")?.to_owned(),
        intent,
        response_kind: row.string("responseKind")?.to_owned(),
        response,
        committed_at: row.timestamp("committedAt")?,
        expires_at: row.timestamp("expiresAt")?,
    })
}

/// Whether a receipt is still readable at `now`.
///
/// This is the fence, not the TTL attribute: AWS deletes a TTL'd row within 48
/// hours, not at the instant, so a reader that trusted TTL would replay an
/// expired receipt for up to two days.
#[must_use]
pub fn receipt_is_live(receipt: &Receipt, now: Timestamp) -> bool {
    now.unix_millis() < receipt.expires_at.unix_millis()
}

#[cfg(test)]
mod tests {
    use aex_wire::ids::{
        AgentId, MessageId, OrganizationId, PrefixedId, RunId, SessionId, Uuid7, WorkspaceId,
    };
    use aex_wire::types::Timestamp;

    use super::{
        SESSION_HEAD, decode_event, decode_head, decode_message, decode_run, encode_event,
        encode_head, encode_message, encode_run,
    };
    use crate::attr::CodecError;
    use crate::keys;
    use crate::wire_pending::{
        Body, Message, Run, SessionEvent, SessionHead, SessionLifecycle, SessionStatus,
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

    #[test]
    fn an_event_identity_must_be_an_observation_id() {
        let event = SessionEvent {
            event_seq: 1,
            event_id: "obs_01j0000000000000000000000".to_owned(),
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

        encoded.insert("eventId".to_owned(), crate::attr::s("evt_1"));
        let error = decode_event(&encoded).expect_err("not an observation id");
        assert!(matches!(error, CodecError::Malformed { .. }), "{error}");
    }
}
